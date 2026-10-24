use std::path::Path;

use copycat_crypto::{CryptoError, DeviceId, DeviceIdentity, aead, identity::SECRET_LEN, kdf};
use copycat_proto::app;
use copycat_sync::{ClipItem, Effect, Hlc, Mark, Snapshot};
use prost::Message;
use rusqlite::{Connection, OptionalExtension, params};
use thiserror::Error;
use zeroize::Zeroizing;

const SCHEMA_VERSION: i64 = 1;
const KEY_CHECK_PLAINTEXT: &[u8] = b"copycat key check v1";

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("数据库错误: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("主密钥不正确或数据已损坏")]
    WrongKey,
    #[error("数据损坏: {0}")]
    Corrupt(&'static str),
    #[error("数据库版本 {0} 高于当前程序支持的版本")]
    TooNew(i64),
}

impl From<CryptoError> for StoreError {
    fn from(_: CryptoError) -> Self {
        StoreError::WrongKey
    }
}

pub type Result<T, E = StoreError> = core::result::Result<T, E>;

pub struct Store {
    conn: Connection,
    field_key: Zeroizing<[u8; 32]>,
}

impl Store {
    pub fn open(path: impl AsRef<Path>, master_key: &[u8; 32]) -> Result<Self> {
        Self::init(Connection::open(path)?, master_key)
    }

    pub fn open_in_memory(master_key: &[u8; 32]) -> Result<Self> {
        Self::init(Connection::open_in_memory()?, master_key)
    }

    fn init(conn: Connection, master_key: &[u8; 32]) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA secure_delete = ON;
             PRAGMA foreign_keys = ON;",
        )?;
        let store = Self { conn, field_key: Zeroizing::new(kdf::derive_key(kdf::purpose::STORE_FIELD, master_key)) };
        store.migrate()?;
        store.check_key()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let version: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::TooNew(version));
        }
        if version < 1 {
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE IF NOT EXISTS meta (
                     key   TEXT PRIMARY KEY,
                     value BLOB NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS roster_ops (
                     hash BLOB PRIMARY KEY,
                     raw  BLOB NOT NULL
                 );
                 CREATE TABLE IF NOT EXISTS items (
                     origin      BLOB    NOT NULL,
                     seq         INTEGER NOT NULL,
                     hlc_wall    INTEGER NOT NULL,
                     hlc_counter INTEGER NOT NULL,
                     body        BLOB    NOT NULL,
                     PRIMARY KEY (origin, seq)
                 );
                 CREATE INDEX IF NOT EXISTS items_by_time ON items (hlc_wall DESC, hlc_counter DESC);
                 PRAGMA user_version = 1;
                 COMMIT;",
            )?;
        }
        Ok(())
    }

    fn check_key(&self) -> Result<()> {
        match self.meta_raw("key_check")? {
            Some(ct) => {
                let pt = aead::open(&self.field_key, b"meta:key_check", &ct).map_err(|_| StoreError::WrongKey)?;
                if pt != KEY_CHECK_PLAINTEXT {
                    return Err(StoreError::WrongKey);
                }
            }
            None => {
                let ct = aead::seal(&self.field_key, b"meta:key_check", KEY_CHECK_PLAINTEXT)?;
                self.set_meta_raw("key_check", &ct)?;
            }
        }
        Ok(())
    }

    fn meta_raw(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    fn set_meta_raw(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
            params![key, value],
        )?;
        Ok(())
    }

    fn get_secret(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let Some(ct) = self.meta_raw(key)? else { return Ok(None) };
        let aad = format!("meta:{key}");
        Ok(Some(Zeroizing::new(aead::open(&self.field_key, aad.as_bytes(), &ct)?)))
    }

    fn set_secret(&self, key: &str, value: &[u8]) -> Result<()> {
        let aad = format!("meta:{key}");
        let ct = aead::seal(&self.field_key, aad.as_bytes(), value)?;
        self.set_meta_raw(key, &ct)
    }

    pub fn save_identity(&self, identity: &DeviceIdentity) -> Result<()> {
        self.set_secret("identity", &identity.export_secret()[..])
    }

    pub fn load_identity(&self) -> Result<Option<DeviceIdentity>> {
        let Some(secret) = self.get_secret("identity")? else { return Ok(None) };
        let arr: &[u8; SECRET_LEN] = secret.as_slice().try_into().map_err(|_| StoreError::Corrupt("身份长度"))?;
        Ok(Some(DeviceIdentity::from_secret(arr).map_err(|_| StoreError::Corrupt("身份"))?))
    }

    pub fn set_setting(&self, key: &str, value: &[u8]) -> Result<()> {
        self.set_secret(&format!("setting:{key}"), value)
    }

    pub fn setting(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.get_secret(&format!("setting:{key}"))?.map(|v| v.to_vec()))
    }

    pub fn set_group(&self, group_id: &[u8; 32]) -> Result<()> {
        self.set_meta_raw("group_id", group_id)
    }

    pub fn group(&self) -> Result<Option<[u8; 32]>> {
        match self.meta_raw("group_id")? {
            Some(v) => Ok(Some(v.as_slice().try_into().map_err(|_| StoreError::Corrupt("group_id"))?)),
            None => Ok(None),
        }
    }

    pub fn add_roster_ops(&mut self, ops: &[Vec<u8>]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("INSERT OR IGNORE INTO roster_ops (hash, raw) VALUES (?1, ?2)")?;
            for raw in ops {
                stmt.execute(params![&kdf::hash(raw)[..], raw])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn roster_ops(&self) -> Result<Vec<Vec<u8>>> {
        let mut stmt = self.conn.prepare("SELECT raw FROM roster_ops ORDER BY hash")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    fn item_aad(origin: &DeviceId, seq: u64) -> Vec<u8> {
        let mut aad = b"items:".to_vec();
        aad.extend_from_slice(&origin.0);
        aad.extend_from_slice(&seq.to_be_bytes());
        aad
    }

    pub fn put_items<'a>(&mut self, items: impl IntoIterator<Item = &'a ClipItem>) -> Result<()> {
        let key = self.field_key.clone();
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO items (origin, seq, hlc_wall, hlc_counter, body) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for it in items {
                let pt = Zeroizing::new(it.to_proto().encode_to_vec());
                let ct = aead::seal(&key, &Self::item_aad(&it.origin, it.seq), &pt)?;
                stmt.execute(params![
                    &it.origin.0[..],
                    it.seq as i64,
                    it.hlc.wall_ms as i64,
                    it.hlc.counter as i64,
                    ct
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn update_item(&mut self, it: &ClipItem) -> Result<()> {
        let pt = Zeroizing::new(it.to_proto().encode_to_vec());
        let ct = aead::seal(&self.field_key, &Self::item_aad(&it.origin, it.seq), &pt)?;
        self.conn.execute(
            "UPDATE items SET body = ?3 WHERE origin = ?1 AND seq = ?2",
            params![&it.origin.0[..], it.seq as i64, ct],
        )?;
        Ok(())
    }

    pub fn delete_items(&mut self, keys: &[(DeviceId, u64)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM items WHERE origin = ?1 AND seq = ?2")?;
            for (o, s) in keys {
                stmt.execute(params![&o.0[..], *s as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn items(&self) -> Result<Vec<ClipItem>> {
        let mut stmt =
            self.conn.prepare("SELECT origin, seq, body FROM items ORDER BY hlc_wall DESC, hlc_counter DESC")?;
        let rows =
            stmt.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?, r.get::<_, Vec<u8>>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (origin, seq, ct) = row?;
            let origin = DeviceId::from_slice(&origin).map_err(|_| StoreError::Corrupt("origin"))?;
            let pt = Zeroizing::new(aead::open(&self.field_key, &Self::item_aad(&origin, seq as u64), &ct)?);
            let p = app::ClipItem::decode(pt.as_slice()).map_err(|_| StoreError::Corrupt("条目"))?;
            let item = ClipItem::from_proto(p).map_err(|_| StoreError::Corrupt("条目"))?;
            if item.origin != origin || item.seq != seq as u64 {
                return Err(StoreError::Corrupt("条目主键不符"));
            }
            out.push(item);
        }
        Ok(out)
    }

    pub fn clear_items(&mut self) -> Result<()> {
        self.conn.execute("DELETE FROM items", [])?;
        Ok(())
    }

    pub fn save_snapshot(&self, s: &Snapshot) -> Result<()> {
        let p = SnapshotProto {
            next_seq: s.next_seq,
            clock_wall: s.clock.0,
            clock_counter: s.clock.1,
            marks: s
                .marks
                .iter()
                .map(|(o, m)| app::Watermark {
                    origin: o.0.to_vec(),
                    contiguous: m.contiguous,
                    extra: m.extra.iter().copied().collect(),
                })
                .collect(),
            applied: s.applied.map(|h| app::Hlc { wall_ms: h.wall_ms, counter: h.counter, node: h.node.0.to_vec() }),
        };
        self.set_secret("engine", &p.encode_to_vec())
    }

    pub fn load_snapshot(&self) -> Result<Option<Snapshot>> {
        let Some(pt) = self.get_secret("engine")? else { return Ok(None) };
        let p = SnapshotProto::decode(pt.as_slice()).map_err(|_| StoreError::Corrupt("同步状态"))?;
        let mut marks = std::collections::BTreeMap::new();
        for w in p.marks {
            let o = DeviceId::from_slice(&w.origin).map_err(|_| StoreError::Corrupt("同步状态"))?;
            marks.insert(o, Mark { contiguous: w.contiguous, extra: w.extra.into_iter().collect() });
        }
        let applied = match p.applied {
            Some(h) => Some(Hlc {
                wall_ms: h.wall_ms,
                counter: h.counter,
                node: DeviceId::from_slice(&h.node).map_err(|_| StoreError::Corrupt("同步状态"))?,
            }),
            None => None,
        };
        Ok(Some(Snapshot { next_seq: p.next_seq, clock: (p.clock_wall, p.clock_counter), marks, applied }))
    }

    pub fn apply_effects(&mut self, effects: &[Effect]) -> Result<()> {
        let new: Vec<&ClipItem> =
            effects.iter().filter_map(|e| if let Effect::Store(i) = e { Some(i) } else { None }).collect();
        if !new.is_empty() {
            self.put_items(new)?;
        }
        for e in effects {
            if let Effect::Forget(keys) = e {
                self.delete_items(keys)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, ::prost::Message)]
struct SnapshotProto {
    #[prost(uint64, tag = "1")]
    next_seq: u64,
    #[prost(uint64, tag = "2")]
    clock_wall: u64,
    #[prost(uint32, tag = "3")]
    clock_counter: u32,
    #[prost(message, repeated, tag = "4")]
    marks: Vec<app::Watermark>,
    #[prost(message, optional, tag = "5")]
    applied: Option<app::Hlc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use copycat_crypto::{DeviceRecord, Platform, Roster};
    use copycat_sync::{Engine, EngineConfig, Rep};

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("copycat-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("db.sqlite")
    }

    #[test]
    fn identity_roundtrip_and_wrong_key() {
        let path = tmp_path("identity");
        let key = [1u8; 32];
        let id = DeviceIdentity::generate().unwrap();
        {
            let s = Store::open(&path, &key).unwrap();
            assert!(s.load_identity().unwrap().is_none());
            s.save_identity(&id).unwrap();
        }
        let s = Store::open(&path, &key).unwrap();
        assert_eq!(s.load_identity().unwrap().unwrap().id(), id.id());
        drop(s);
        assert!(matches!(Store::open(&path, &[2u8; 32]), Err(StoreError::WrongKey)));
    }

    #[test]
    fn secrets_not_in_plaintext_on_disk() {
        let path = tmp_path("plaintext");
        let key = [3u8; 32];
        let marker = "非常机密的剪贴板内容-7f3a9c";
        {
            let mut s = Store::open(&path, &key).unwrap();
            let mut e = Engine::new(DeviceId([1; 16]), EngineConfig::default());
            let fx = e.local_copy(vec![Rep::from_bytes("text/plain", marker.as_bytes().to_vec())], 10);
            s.apply_effects(&fx).unwrap();
            s.set_setting("relay", b"secret-relay-token-55aa").unwrap();
            s.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
        }
        let mut all = Vec::new();
        for entry in std::fs::read_dir(path.parent().unwrap()).unwrap() {
            all.extend(std::fs::read(entry.unwrap().path()).unwrap());
        }
        let hay = String::from_utf8_lossy(&all);
        assert!(!hay.contains(marker), "剪贴板内容不应以明文出现在磁盘上");
        assert!(!hay.contains("secret-relay-token-55aa"));
    }

    #[test]
    fn items_and_snapshot_roundtrip() {
        let key = [4u8; 32];
        let mut s = Store::open_in_memory(&key).unwrap();
        let me = DeviceId([7; 16]);
        let mut e = Engine::new(me, EngineConfig::default());
        let mut fx = e.local_copy(vec![Rep::from_bytes("text/plain", b"a".to_vec())], 10);
        fx.extend(e.local_copy(vec![Rep::from_bytes("text/plain", b"b".to_vec())], 11));
        s.apply_effects(&fx).unwrap();
        s.save_snapshot(&e.snapshot()).unwrap();

        let items = s.items().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].reps[0].inline.as_deref(), Some(&b"b"[..]), "按时间从新到旧");
        assert_eq!(s.load_snapshot().unwrap().unwrap(), e.snapshot());

        let restored = Engine::restore(me, EngineConfig::default(), s.load_snapshot().unwrap().unwrap(), items);
        assert_eq!(restored.snapshot(), e.snapshot());

        s.apply_effects(&[Effect::Forget(vec![(me, 1)])]).unwrap();
        assert_eq!(s.items().unwrap().len(), 1);

        let mut changed = s.items().unwrap().remove(0);
        changed.reps.push(Rep::from_bytes("text/html", b"<b>b</b>".to_vec()));
        s.update_item(&changed).unwrap();
        assert_eq!(s.items().unwrap(), vec![changed]);
        let mut gone = e.history(0).into_iter().find(|i| i.seq == 1).unwrap().clone();
        gone.reps.clear();
        gone.reps.push(Rep::from_bytes("text/plain", b"x".to_vec()));
        s.update_item(&gone).unwrap();
        assert_eq!(s.items().unwrap().len(), 1);
    }

    #[test]
    fn swapped_ciphertext_rows_detected() {
        let key = [5u8; 32];
        let mut s = Store::open_in_memory(&key).unwrap();
        let mut e = Engine::new(DeviceId([1; 16]), EngineConfig::default());
        let mut fx = e.local_copy(vec![Rep::from_bytes("text/plain", b"a".to_vec())], 10);
        fx.extend(e.local_copy(vec![Rep::from_bytes("text/plain", b"b".to_vec())], 11));
        s.apply_effects(&fx).unwrap();

        s.conn.execute("UPDATE items SET body = (SELECT body FROM items WHERE seq = 2) WHERE seq = 1", []).unwrap();
        assert!(s.items().is_err());
    }

    #[test]
    fn roster_persistence() {
        let key = [6u8; 32];
        let mut s = Store::open_in_memory(&key).unwrap();
        let id = DeviceIdentity::generate().unwrap();
        let rec = DeviceRecord::issue(&id, "pc", Platform::Windows, 1).unwrap();
        let (roster, _) = Roster::create(&id, &rec, 1).unwrap();
        s.set_group(roster.group_id()).unwrap();
        s.add_roster_ops(&roster.ops_raw()).unwrap();
        s.add_roster_ops(&roster.ops_raw()).unwrap();
        let ops = s.roster_ops().unwrap();
        let back = Roster::from_ops(&s.group().unwrap().unwrap(), ops.iter().map(Vec::as_slice)).unwrap();
        assert_eq!(back.digest(), roster.digest());
    }
}
