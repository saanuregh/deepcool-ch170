//! Self-managed autostart via a Windows "at logon" scheduled task.
//!
//! Because the app carries a `requireAdministrator` manifest, it can't be
//! launched from the Startup folder without a UAC prompt each logon. A scheduled
//! task with "run with highest privileges" starts it elevated and silently. These
//! subcommands let the binary install/remove/inspect that task itself, pointing
//! at its own current path — no external script needed.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::Command;

pub const TASK_NAME: &str = "DeepCool CH170";

/// Register (or replace) the logon task pointing at this executable.
pub fn install() -> Result<()> {
    let exe = std::env::current_exe().context("resolve current executable path")?;
    let exe_str = exe.to_string_lossy().to_string();
    let user = current_user();

    if exe_str.to_lowercase().contains(r"\target\") {
        // Running straight from a build directory would register a fragile path.
        eprintln!(
            "WARNING: installing autostart pointing into a build directory:\n  {exe_str}\n\
             Consider running this from the installed location (e.g. C:\\bin\\deepcool-ch170.exe)."
        );
    }

    let xml = build_task_xml(&exe_str, &user);
    let xml_path = std::env::temp_dir().join("deepcool-ch170-task.xml");
    write_utf16le(&xml_path, &xml).context("write task definition XML")?;

    let output = Command::new("schtasks")
        .args([
            "/Create",
            "/TN",
            TASK_NAME,
            "/XML",
            &xml_path.to_string_lossy(),
            "/F",
        ])
        .output()
        .context("run schtasks /Create")?;

    let _ = std::fs::remove_file(&xml_path);

    if !output.status.success() {
        bail!(
            "schtasks /Create failed ({}): {}",
            output.status,
            decode(&output.stderr).trim()
        );
    }

    notify(
        TASK_NAME,
        &format!(
            "Autostart installed.\n\nRuns at logon (elevated, no UAC prompt) as:\n{user}\n\nTarget:\n{exe_str}"
        ),
    );
    Ok(())
}

/// Remove the logon task.
pub fn uninstall() -> Result<()> {
    let output = Command::new("schtasks")
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output()
        .context("run schtasks /Delete")?;

    if !output.status.success() {
        let msg = decode(&output.stderr).to_lowercase();
        // Treat "task does not exist" as success (idempotent uninstall).
        if msg.contains("cannot find") || msg.contains("does not exist") {
            notify(
                TASK_NAME,
                "Autostart was not installed (nothing to remove).",
            );
            return Ok(());
        }
        bail!(
            "schtasks /Delete failed ({}): {}",
            output.status,
            msg.trim()
        );
    }

    notify(TASK_NAME, "Autostart removed.");
    Ok(())
}

/// Report whether the task is installed and what it points at.
pub fn status() -> Result<()> {
    let output = Command::new("schtasks")
        .args(["/Query", "/TN", TASK_NAME, "/V", "/FO", "LIST"])
        .output()
        .context("run schtasks /Query")?;

    if !output.status.success() {
        notify(TASK_NAME, "Autostart is NOT installed.");
        return Ok(());
    }

    let text = decode(&output.stdout);
    let target = text
        .lines()
        .find(|l| l.trim_start().to_lowercase().starts_with("task to run:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_else(|| "(unknown)".to_string());

    notify(
        TASK_NAME,
        &format!("Autostart is INSTALLED.\n\nTarget:\n{target}"),
    );
    Ok(())
}

/// Attach to the parent process's console (if any) so `println!`/`eprintln!`
/// from these CLI subcommands is visible when launched from a terminal. A no-op
/// (harmless failure) when there is no parent console.
pub fn attach_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: no arguments to invalidate; failure is ignored by design.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// `DOMAIN\User` for the current user (falls back to just the user name).
fn current_user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
        _ => name,
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A complete Task Scheduler definition: logon trigger, highest privileges, no
/// execution time limit, runs on battery. Matches what we set up by hand.
fn build_task_xml(exe: &str, user: &str) -> String {
    let exe = xml_escape(exe);
    let user = xml_escape(user);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Drives the DeepCool CH170 display with live sensor data.</Description>
    <URI>\{TASK_NAME}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <!-- Relaunch if the process exits with an error (e.g. PawnIO/NVIDIA not yet
         ready at logon), so a boot-time race doesn't leave the display dead. -->
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// schtasks /XML wants a Unicode file; write UTF-16LE with a BOM.
fn write_utf16le(path: &std::path::Path, contents: &str) -> Result<()> {
    let mut bytes = vec![0xFF, 0xFE]; // UTF-16 LE BOM
    for unit in contents.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(&bytes)?;
    Ok(())
}

/// Decode schtasks output, honoring a UTF-16 LE BOM, else lossy UTF-8.
fn decode(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

/// Print the result to stderr and, when no console is attached (double-clicked),
/// also show a message box — so it's visible however the command was launched.
fn notify(title: &str, body: &str) {
    println!("{title}: {body}");
    let _ = std::io::stdout().flush();

    // A console (terminal / mise) means the printed text is enough; otherwise
    // (double-clicked, no console) pop a dialog.
    use windows::Win32::System::Console::GetConsoleWindow;
    if unsafe { !GetConsoleWindow().0.is_null() } {
        return;
    }

    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    use windows::core::HSTRING;
    // SAFETY: the HSTRINGs are valid null-terminated UTF-16 for the call.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(body),
            &HSTRING::from(title),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
