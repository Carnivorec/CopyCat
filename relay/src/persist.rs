use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub type RosterFile = ([u8; 32], Vec<Vec<u8>>);

pub struct DataDir(PathBuf);

impl DataDir {
    pub fn new(p: impl Into<PathBuf>) -> Result<Self> {
        let p = p.into();
        fs::create_dir_all(&p).with_context(|| format!("无法创建数据目录 {}", p.display()))?;
        Ok(Self(p))
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write_atomic(&self, name: &str, data: &[u8]) -> Result<()> {
        let tmp = self.path(&format!("{name}.tmp"));
        fs::write(&tmp, data)?;
        restrict_permissions(&tmp);
        fs::rename(&tmp, self.path(name))?;
        Ok(())
    }

    pub fn load_roster(&self) -> Result<Option<RosterFile>> {
        let p = self.path("roster.bin");
        if !p.exists() {
            return Ok(None);
        }
        let data = fs::read(p)?;
        if data.len() < 32 {
            bail!("roster.bin 损坏");
        }
        let group: [u8; 32] = data[..32].try_into()?;
        let mut ops = Vec::new();
        let mut rest = &data[32..];
        while !rest.is_empty() {
            if rest.len() < 4 {
                bail!("roster.bin 损坏");
            }
            let len = u32::from_be_bytes(rest[..4].try_into()?) as usize;
            if rest.len() < 4 + len {
                bail!("roster.bin 损坏");
            }
            ops.push(rest[4..4 + len].to_vec());
            rest = &rest[4 + len..];
        }
        Ok(Some((group, ops)))
    }

    pub fn save_roster(&self, group: &[u8; 32], ops: &[Vec<u8>]) -> Result<()> {
        let mut data = group.to_vec();
        for op in ops {
            data.extend_from_slice(&(op.len() as u32).to_be_bytes());
            data.extend_from_slice(op);
        }
        self.write_atomic("roster.bin", &data)
    }
}

#[cfg(unix)]
fn restrict_permissions(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_p: &Path) {}
