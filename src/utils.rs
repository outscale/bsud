use easy_error::format_err;
use log::trace;
use std::error::Error;
use std::process::Command;
use std::process::Stdio;

const NB_OF_BYTES_IN_GIB: usize = 1024_usize.pow(3);

pub fn bytes_to_gib(bytes: usize) -> f32 {
    bytes as f32 / NB_OF_BYTES_IN_GIB as f32
}

pub fn bytes_to_gib_rounded(bytes: usize) -> usize {
    bytes_to_gib(bytes).ceil() as usize
}

pub fn gib_to_bytes(gib: usize) -> usize {
    gib * NB_OF_BYTES_IN_GIB
}

pub struct ExecOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

fn cmd_str(cmd: &str, args: &[&str]) -> String {
    let mut concatenated_arg = String::from(cmd);
    for arg in args {
        concatenated_arg += " ";
        concatenated_arg += arg;
    }
    concatenated_arg
}

fn exec_raw(cmd: &str, args: &[&str]) -> Result<ExecOutput, Box<dyn Error>> {
    let cmd_str = cmd_str(cmd, args);
    trace!("exec {}", cmd_str);
    let output = Command::new(cmd)
        .args(args)
        .stdout(Stdio::piped())
        .stdout(Stdio::piped())
        .output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let stderr = String::from_utf8(output.stderr)?;
    let success = output.status.success();
    if !success {
        if !stdout.is_empty() {
            trace!("{} stdout: {}", cmd_str, stdout);
        }
        if !stderr.is_empty() {
            trace!("{} stderr: {}", cmd_str, stderr);
        }
    }
    Ok(ExecOutput {
        success,
        stdout,
        stderr,
    })
}

pub fn exec(cmd: &str, args: &[&str]) -> Result<ExecOutput, Box<dyn Error>> {
    let output = exec_raw(cmd, args)?;
    if !output.success {
        return Err(Box::new(format_err!("{} {:?} exited non zero", cmd, args)));
    }
    Ok(output)
}

pub fn exec_bool(cmd: &str, args: &[&str]) -> Result<bool, Box<dyn Error>> {
    let output = exec_raw(cmd, args)?;
    Ok(output.success)
}
