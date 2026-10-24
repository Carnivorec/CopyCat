use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

const MAX: u64 = 2 * 1024 * 1024;

pub struct LogFile {
    path: PathBuf,
    file: File,
    size: u64,
}

pub fn open() -> Option<LogFile> {
    let dir = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("app.copycat.desktop");
    fs::create_dir_all(&dir).ok()?;
    LogFile::open(dir.join("copycat.log")).ok()
}

fn append(path: &PathBuf) -> io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}

impl LogFile {
    fn open(path: PathBuf) -> io::Result<Self> {
        let file = append(&path)?;
        let size = file.metadata().map_or(0, |m| m.len());
        let mut f = Self { path, file, size };
        if f.size >= MAX {
            f.rotate();
        }
        Ok(f)
    }

    fn rotate(&mut self) {
        let old = self.path.with_extension("log.old");
        let failed = fs::rename(&self.path, &old).err();
        let reopened = match failed {
            None => append(&self.path),
            Some(_) => OpenOptions::new().create(true).write(true).truncate(true).open(&self.path),
        };
        if let Ok(f) = reopened {
            self.file = f;
        }

        self.size = 0;
        if let Some(e) = failed {
            let _ = writeln!(self, "（上一份日志没能改名为 copycat.log.old：{e}，已清空重写）");
        }
    }
}

impl Write for LogFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size + buf.len() as u64 > MAX {
            self.rotate();
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotates_while_running() {
        let dir = std::env::temp_dir().join(format!("copycat-log-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("copycat.log");
        let mut log = LogFile::open(path.clone()).unwrap();
        let line = vec![b'x'; 64 * 1024];
        for _ in 0..80 {
            log.write_all(&line).unwrap();
        }
        drop(log);
        let cur = fs::metadata(&path).unwrap().len();
        let old = fs::metadata(path.with_extension("log.old")).unwrap().len();
        assert!(cur <= MAX && old <= MAX, "日志超过上限：{cur} / {old}");
        fs::remove_file(&path).unwrap();
        fs::remove_file(path.with_extension("log.old")).unwrap();
        fs::remove_dir(&dir).unwrap();
    }
}
