/* local_mpd.rs
 *
 * A Music Player Daemon of Flaclify's own, for people who do not run one.
 *
 * Flaclify is an MPD client, and MPD is the one thing a person downloading a music player
 * does not have. With the `managed-mpd` setting on, Flaclify writes a small mpd.conf under its
 * own data directory, starts `mpd --no-daemon` on the chosen music folder, listens on a private
 * local socket, points its own connection settings at that socket, and stops the daemon when it
 * quits. Nothing of the user's own MPD, if they have one, is touched: different socket, different
 * database, different state file.
 *
 * The daemon comes from `$FLACLIFY_MPD`, then `/app/bin/mpd` inside a Flatpak (the release
 * manifest builds one), then `mpd` on the PATH.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use crate::utils::{is_flatpak, settings_manager};
use gtk::{glib, prelude::*};
use std::{
    fs,
    io::Write,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Whether Flaclify is asked to run the daemon itself.
pub fn is_managed() -> bool {
    settings_manager().child("client").boolean("managed-mpd")
}

/// The mpd program Flaclify would start, if there is one.
pub fn binary() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("FLACLIFY_MPD") {
        let path = PathBuf::from(explicit);
        if is_executable(&path) {
            return Some(path);
        }
    }
    if is_flatpak() {
        let bundled = PathBuf::from("/app/bin/mpd");
        if is_executable(&bundled) {
            return Some(bundled);
        }
    }
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("mpd"))
                .find(|candidate| is_executable(candidate))
        })
        .or_else(|| {
            ["/usr/bin/mpd", "/usr/local/bin/mpd"]
                .into_iter()
                .map(PathBuf::from)
                .find(|candidate| is_executable(candidate))
        })
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The folder the built-in player serves: the setting, else the XDG music folder, else ~/Music.
pub fn music_dir() -> PathBuf {
    let configured = settings_manager()
        .child("client")
        .string("managed-mpd-music-dir");
    if !configured.trim().is_empty() {
        return PathBuf::from(configured.trim());
    }
    default_music_dir()
}

pub fn default_music_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Music)
        .unwrap_or_else(|| glib::home_dir().join("Music"))
}

/// Where the daemon keeps its database, state, stickers, playlists and log.
pub fn data_dir() -> PathBuf {
    glib::user_data_dir().join("flaclify").join("mpd")
}

/// The private socket the daemon listens on.
pub fn socket_path() -> PathBuf {
    glib::user_runtime_dir().join("flaclify").join("mpd.sock")
}

/// The socket as `$MPD_HOST` for programs Flaclify spawns (flacli, the agent), so they find
/// the same daemon without configuration.
pub fn mpd_host_env() -> Option<String> {
    if is_managed() {
        Some(socket_path().to_string_lossy().into_owned())
    } else {
        None
    }
}

fn socket_alive() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

/// The output plugins this mpd was built with, from `mpd --version`.
fn output_plugins(bin: &Path) -> Vec<String> {
    let Ok(output) = Command::new(bin).arg("--version").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim_end().eq_ignore_ascii_case("output plugins:") {
            return lines
                .next()
                .map(|l| l.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default();
        }
    }
    Vec::new()
}

fn quote(path: &Path) -> String {
    format!("\"{}\"", path.to_string_lossy().replace('"', "\\\""))
}

/// Write mpd.conf and return its path. A changed music folder drops the old database so the
/// daemon builds a fresh one rather than serving stale paths.
fn write_config(bin: &Path) -> Result<PathBuf, String> {
    let data = data_dir();
    let playlists = data.join("playlists");
    fs::create_dir_all(&playlists).map_err(|e| format!("cannot create {}: {e}", data.display()))?;
    if let Some(parent) = socket_path().parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }

    let music = music_dir();
    if !music.is_dir() {
        fs::create_dir_all(&music)
            .map_err(|e| format!("cannot create {}: {e}", music.display()))?;
    }
    let marker = data.join("music_dir");
    let previous = fs::read_to_string(&marker).unwrap_or_default();
    if previous.trim() != music.to_string_lossy() {
        let _ = fs::remove_file(data.join("database"));
        let _ = fs::write(&marker, music.to_string_lossy().as_bytes());
    }

    let plugins = output_plugins(bin);
    let output = ["pipewire", "pulse", "alsa"]
        .into_iter()
        .find(|p| plugins.iter().any(|have| have == p))
        .unwrap_or("pipewire");
    let output_name = match output {
        "pipewire" => "PipeWire",
        "pulse" => "PulseAudio",
        _ => "ALSA",
    };

    let mut conf = String::new();
    conf.push_str("# Written by Flaclify. Edits are overwritten at every start.\n");
    conf.push_str(&format!("music_directory     {}\n", quote(&music)));
    conf.push_str(&format!("playlist_directory  {}\n", quote(&playlists)));
    conf.push_str(&format!(
        "db_file             {}\n",
        quote(&data.join("database"))
    ));
    conf.push_str(&format!(
        "state_file          {}\n",
        quote(&data.join("state"))
    ));
    conf.push_str(&format!(
        "sticker_file        {}\n",
        quote(&data.join("sticker.sql"))
    ));
    conf.push_str(&format!(
        "log_file            {}\n",
        quote(&data.join("mpd.log"))
    ));
    conf.push_str(&format!("bind_to_address     {}\n", quote(&socket_path())));
    conf.push_str("auto_update             \"yes\"\n");
    conf.push_str("restore_paused          \"yes\"\n");
    conf.push_str("follow_outside_symlinks \"yes\"\n");
    conf.push_str("follow_inside_symlinks  \"yes\"\n");
    conf.push_str("metadata_to_use         \"+comment\"\n");
    conf.push_str(&format!(
        "audio_output {{\n    type \"{output}\"\n    name \"{output_name}\"\n}}\n"
    ));

    let path = data.join("mpd.conf");
    let mut file =
        fs::File::create(&path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    file.write_all(conf.as_bytes())
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// Point Flaclify's own connection at the daemon's socket.
pub fn apply_connection_settings() {
    let settings = settings_manager().child("client");
    let _ = settings.set_boolean("mpd-use-unix-socket", true);
    let _ = settings.set_string("mpd-unix-socket", &socket_path().to_string_lossy());
}

/// Spawn the daemon unless one of ours already answers on the socket. Returns at once; the
/// socket comes up a moment later (`ensure_running` waits for it).
///
/// Must run on the main thread: the daemon is told to die with the thread that forked it
/// (`PR_SET_PDEATHSIG`), so a Flaclify that crashes or is killed never leaves an mpd behind.
pub fn start() -> Result<(), String> {
    apply_connection_settings();
    if socket_alive() {
        return Ok(());
    }
    if let Some(child) = CHILD.lock().unwrap().as_mut()
        && child.try_wait().is_ok_and(|s| s.is_none())
    {
        // Ours, still starting up.
        return Ok(());
    }
    let bin = binary().ok_or_else(|| {
        "no mpd program was found on this computer; install the mpd package, or connect to an MPD you run".to_owned()
    })?;
    let conf = write_config(&bin)?;

    // A previous instance that died without cleaning up leaves a dead socket; mpd unlinks it.
    let mut command = Command::new(&bin);
    command
        .arg("--no-daemon")
        .arg(&conf)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    // SAFETY: prctl between fork and exec touches nothing but the child's own flags.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
    let pid = child.id();
    *CHILD.lock().unwrap() = Some(child);
    eprintln!(
        "Started mpd (pid {pid}) from {} on {}",
        bin.display(),
        music_dir().display()
    );
    watch(pid);
    Ok(())
}

/// Reap the daemon if it dies on its own, say so, and forget it, so the next refresh starts a
/// fresh one instead of trusting a dead socket.
fn watch(pid: u32) {
    std::thread::Builder::new()
        .name("mpd-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let mut guard = CHILD.lock().unwrap();
                let Some(child) = guard.as_mut() else { return };
                if child.id() != pid {
                    return;
                }
                match child.try_wait() {
                    Ok(Some(status)) => {
                        eprintln!(
                            "Built-in player: mpd (pid {pid}) exited on its own: {status}; see {}",
                            data_dir().join("mpd.log").display()
                        );
                        *guard = None;
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => return,
                }
            }
        })
        .ok();
}

/// Start the daemon if needed and wait, without blocking the main loop, until its socket
/// answers or a few seconds have passed.
pub async fn ensure_running() -> Result<(), String> {
    start()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if socket_alive() {
            return Ok(());
        }
        if CHILD.lock().unwrap().is_none() {
            return Err(format!(
                "mpd exited at once; its log is {}",
                data_dir().join("mpd.log").display()
            ));
        }
        glib::timeout_future(Duration::from_millis(100)).await;
    }
    Err(format!(
        "mpd did not open {} in time; its log is {}",
        socket_path().display(),
        data_dir().join("mpd.log").display()
    ))
}

/// Stop the daemon we started, if any. Other people's MPDs are never touched.
pub fn stop() {
    let Some(mut child) = CHILD.lock().unwrap().take() else {
        return;
    };
    let pid = child.id() as libc::pid_t;
    // SAFETY: plain signal to a child we spawned and still own.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            let _ = fs::remove_file(socket_path());
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(socket_path());
}

/// One line for preferences and onboarding: what would happen with the built-in player on.
pub fn describe() -> String {
    match binary() {
        Some(bin) => format!(
            "Flaclify runs {} on {} and stops it on quit.",
            bin.display(),
            music_dir().display()
        ),
        None => "No mpd program was found on this computer. Install the mpd package, or connect to an MPD you run.".to_owned(),
    }
}
