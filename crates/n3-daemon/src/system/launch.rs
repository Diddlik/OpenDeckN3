//! Starting programs and shell commands without blocking the service.

use std::{
    path::Path,
    process::{Child, Command, Stdio},
};

use anyhow::Context;

/// Waits for the child in the background so it never becomes a zombie.
fn detach(child: Child) {
    std::thread::spawn(move || {
        let mut child = child;
        child.wait().ok();
    });
}

fn is_executable(path: &Path) -> bool {
    #[cfg(windows)]
    {
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["exe", "bat", "cmd", "com"].contains(&e.to_ascii_lowercase().as_str())
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && path
                .metadata()
                .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
    }
}

/// Opens a program, file, folder or URL. `args` are only used for programs.
pub fn launch(target: &str, args: &str) -> anyhow::Result<()> {
    let target = target.trim().trim_matches('"');
    anyhow::ensure!(!target.is_empty(), "kein Programm angegeben");
    let args = shell_words::split(args).context("Argumente ungültig (Anführungszeichen?)")?;
    let path = Path::new(target);

    // Explicit executables, or bare names like `notepad` / `firefox` found via PATH.
    let bare_name = !target.contains(['/', '\\']) && !path.exists();
    if is_executable(path) || bare_name {
        let spawned = Command::new(target)
            .args(&args)
            .stdin(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => {
                detach(child);
                return Ok(());
            }
            Err(err) if !bare_name => {
                return Err(err).with_context(|| format!("„{target}“ startet nicht"));
            }
            Err(_) => {} // not on PATH – let the OS try (e.g. registered apps)
        }
    }
    anyhow::ensure!(
        args.is_empty(),
        "Argumente gehen nur bei Programmen (.exe bzw. ausführbaren Dateien)"
    );
    open::that_detached(target).with_context(|| format!("„{target}“ lässt sich nicht öffnen"))
}

/// Runs a command line in the system shell (`cmd /C` on Windows, `sh -c` elsewhere).
pub fn run_command(command: &str) -> anyhow::Result<()> {
    let command = command.trim();
    anyhow::ensure!(!command.is_empty(), "kein Befehl angegeben");

    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = Command::new("cmd");
        cmd.arg("/C")
            .raw_arg(command)
            .creation_flags(CREATE_NO_WINDOW);
        cmd
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    };

    if let Some(home) = dirs::home_dir() {
        cmd.current_dir(home);
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("Befehl startet nicht")?;
    detach(child);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_shell_commands() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("out.txt");
        run_command(&format!("echo hallo > \"{}\"", file.display())).unwrap();
        for _ in 0..50 {
            if file.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(std::fs::read_to_string(&file).unwrap().contains("hallo"));
        assert!(run_command("  ").is_err());
    }

    #[test]
    fn rejects_bad_launch_input() {
        assert!(launch("", "").is_err());
        assert!(launch("notepad", "\"unclosed").is_err());
    }
}
