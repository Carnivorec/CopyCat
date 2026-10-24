use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::Settings;

const DAY: Duration = Duration::from_secs(24 * 60 * 60);
const MB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub max_age: Option<Duration>,

    pub max_count: Option<usize>,

    pub max_bytes: Option<u64>,
}

impl Limits {
    pub fn from_settings(s: &Settings) -> Self {
        Self {
            max_age: (s.screenshot_keep_days > 0).then(|| DAY * s.screenshot_keep_days),
            max_count: (s.screenshot_max_count > 0).then_some(s.screenshot_max_count as usize),
            max_bytes: (s.screenshot_max_mb > 0).then(|| u64::from(s.screenshot_max_mb) * MB),
        }
    }

    pub fn is_unlimited(&self) -> bool {
        *self == Self::default()
    }
}

fn is_shot_name(name: &str) -> bool {
    name.starts_with("截图_") && name.get(name.len() - 4..).is_some_and(|ext| ext.eq_ignore_ascii_case(".png"))
}

pub fn prune(dir: &Path, limits: &Limits, now: SystemTime) -> Vec<PathBuf> {
    if limits.is_unlimited() {
        return Vec::new();
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut shots: Vec<(PathBuf, SystemTime, u64)> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter(|e| e.file_name().to_str().is_some_and(is_shot_name))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.modified().unwrap_or(now), m.len()))
        })
        .collect();

    shots.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| b.0.cmp(&a.0)));

    let mut removed = Vec::new();
    let mut bytes = 0u64;
    let mut over_size = false;
    for (i, (path, modified, len)) in shots.into_iter().enumerate() {
        bytes = bytes.saturating_add(len);

        over_size |= i > 0 && limits.max_bytes.is_some_and(|max| bytes > max);
        let too_old = limits.max_age.is_some_and(|age| now.duration_since(modified).is_ok_and(|d| d > age));
        let too_many = limits.max_count.is_some_and(|n| i >= n);
        if (too_old || too_many || over_size) && std::fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(dir: &Path, name: &str, size: usize, age_hours: u64, now: SystemTime) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, vec![0u8; size]).unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        f.set_modified(now - Duration::from_secs(age_hours * 3600)).unwrap();
        p
    }

    #[test]
    fn names() {
        assert!(is_shot_name("截图_20261005_141502.png"));
        assert!(is_shot_name("截图_20261005_141502 (1).PNG"));
        assert!(!is_shot_name("截图_20261005.jpg"));
        assert!(!is_shot_name("IMG_0001.png"));
        assert!(!is_shot_name(".copycat-ab-0.part"));
        assert!(!is_shot_name("截图_"));
        assert!(!is_shot_name("截图_图"));
        assert!(!is_shot_name("截图_x图"));
    }

    #[test]
    fn unlimited_or_missing_dir_does_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        shot(dir.path(), "截图_1.png", 10, 1000, now);
        assert!(prune(dir.path(), &Limits::default(), now).is_empty());
        let limits = Limits { max_count: Some(1), ..Limits::default() };
        assert!(prune(&dir.path().join("不存在"), &limits, now).is_empty());
    }

    #[test]
    fn by_age_only_touches_screenshots() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let old = shot(dir.path(), "截图_old.png", 10, 30, now);
        let fresh = shot(dir.path(), "截图_new.png", 10, 2, now);
        let mine = shot(dir.path(), "我的照片.png", 10, 100, now);
        std::fs::create_dir(dir.path().join("截图_目录.png")).unwrap();
        let limits = Limits { max_age: Some(DAY), ..Limits::default() };
        assert_eq!(prune(dir.path(), &limits, now), vec![old.clone()]);
        assert!(!old.exists());
        assert!(fresh.exists() && mine.exists() && dir.path().join("截图_目录.png").is_dir());
    }

    #[test]
    fn by_count_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let ps: Vec<_> = (0..5).map(|i| shot(dir.path(), &format!("截图_{i}.png"), 10, 5 - i, now)).collect();
        let limits = Limits { max_count: Some(2), ..Limits::default() };
        let mut removed = prune(dir.path(), &limits, now);
        removed.sort();
        assert_eq!(removed, ps[..3].to_vec());
        assert!(ps[3].exists() && ps[4].exists());
    }

    #[test]
    fn by_size_deletes_everything_older_than_the_cap_but_keeps_newest() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let a = shot(dir.path(), "截图_a.png", 100, 4, now);
        let b = shot(dir.path(), "截图_b.png", 10, 3, now);
        let c = shot(dir.path(), "截图_c.png", 60, 2, now);
        let d = shot(dir.path(), "截图_d.png", 50, 1, now);
        let limits = Limits { max_bytes: Some(100), ..Limits::default() };
        let mut removed = prune(dir.path(), &limits, now);
        removed.sort();

        assert_eq!(removed, vec![a, b, c]);
        assert!(d.exists());

        let limits = Limits { max_bytes: Some(10), ..Limits::default() };
        assert!(prune(dir.path(), &limits, now).is_empty());
    }

    #[test]
    fn from_settings_zero_means_unlimited() {
        let s = Settings {
            screenshot_keep_days: 1,
            screenshot_max_count: 0,
            screenshot_max_mb: 500,
            ..Settings::default()
        };
        let l = Limits::from_settings(&s);
        assert_eq!(l.max_age, Some(DAY));
        assert_eq!(l.max_count, None);
        assert_eq!(l.max_bytes, Some(500 * MB));
        assert!(Limits::from_settings(&Settings::default()).is_unlimited());
    }
}
