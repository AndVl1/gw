use anyhow::{Context, Result};
use std::fs::{create_dir_all, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

pub struct LogWriter {
    path: PathBuf,
    inner: BufWriter<File>,
}

impl LogWriter {
    pub fn create(dir: Option<&str>) -> Result<Self> {
        let dir = dir.unwrap_or("./build-logs");
        create_dir_all(dir).with_context(|| format!("create log dir {dir}"))?;
        let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S");
        let path = Path::new(dir).join(format!("gw-{stamp}.log"));
        let file = File::create(&path).with_context(|| format!("create log {path:?}"))?;
        Ok(Self {
            path,
            inner: BufWriter::new(file),
        })
    }

    pub fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.inner.write_all(line.as_bytes())?;
        self.inner.write_all(b"\n")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for LogWriter {
    fn drop(&mut self) {
        let _ = self.inner.flush();
    }
}
