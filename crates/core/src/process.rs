use std::{
    io::{Read, Seek, SeekFrom},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

/// Bounded command output without platform-specific `timeout` executables or pipe deadlocks.
pub(crate) trait CommandOutputExt {
    fn bounded_output(&mut self, timeout: Duration) -> std::io::Result<Output>;
}

impl CommandOutputExt for Command {
    fn bounded_output(&mut self, timeout: Duration) -> std::io::Result<Output> {
        let mut stdout = tempfile::tempfile()?;
        let mut stderr = tempfile::tempfile()?;
        let mut child = self
            .stdin(Stdio::null())
            .stdout(stdout.try_clone()?)
            .stderr(stderr.try_clone()?)
            .spawn()?;
        let started = Instant::now();
        let status = (|| loop {
            if let Some(status) = child.try_wait()? {
                break Ok(status);
            }
            if started.elapsed() >= timeout
                || stdout
                    .metadata()?
                    .len()
                    .saturating_add(stderr.metadata()?.len())
                    > 16 * 1024 * 1024
            {
                break Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Command exceeded its time or output limit",
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        })();
        let status = match status {
            Ok(status) => status,
            Err(error) => {
                // ponytail: kills the direct process; OS job groups are needed for descendant cancellation.
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let read = |file: &mut std::fs::File| -> std::io::Result<Vec<u8>> {
            let size = file.metadata()?.len();
            file.seek(SeekFrom::Start(size.saturating_sub(512 * 1024)))?;
            let mut bytes = Vec::new();
            file.take(512 * 1024).read_to_end(&mut bytes)?;
            if size > 512 * 1024 {
                bytes.splice(..0, b"[Showing last 512 KiB]\n".iter().copied());
            }
            Ok(bytes)
        };
        Ok(Output {
            status,
            stdout: read(&mut stdout)?,
            stderr: read(&mut stderr)?,
        })
    }
}
