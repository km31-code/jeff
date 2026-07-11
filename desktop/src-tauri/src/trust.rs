// apex d4: trust ladder. Autonomy is earned per action class, capped for
// sensitive classes, and demoted immediately on reverts or explicit user tap.

use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Result};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};

use crate::{
    action_bus::ActionClass,
    models::{ActionReceiptDto, TrustLevelDto},
    store::TaskStore,
};

pub const TRUST_LEVEL_L1: &str = "L1";
pub const TRUST_LEVEL_L2: &str = "L2";
pub const TRUST_LEVEL_L3: &str = "L3";
pub const TRUST_GRADUATION_STREAK: i64 = 10;
pub const HARD_CAP_ACTION_CLASSES: &[&str] = &["email.send", "file.delete"];
pub const HARD_CAP_TOOL_PREFIX: &str = "tool.custom.";
pub const DEFAULT_TRUST_CLASSES: &[&str] = &[
    "doc.insert",
    "doc.replace",
    "doc.suggest",
    "file.write",
    "file.delete",
    "email.draft",
    "email.send",
    "calendar.propose",
    "system.open",
    "tool.custom.*",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TrustLevel {
    L1,
    L2,
    L3,
}

impl TrustLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L1 => TRUST_LEVEL_L1,
            Self::L2 => TRUST_LEVEL_L2,
            Self::L3 => TRUST_LEVEL_L3,
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim() {
            TRUST_LEVEL_L1 => Ok(Self::L1),
            TRUST_LEVEL_L2 => Ok(Self::L2),
            TRUST_LEVEL_L3 => Ok(Self::L3),
            other => Err(anyhow!("invalid trust level: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
struct TrustLevelRecord {
    class: String,
    level: TrustLevel,
    approval_streak: i64,
    graduation_offered_at: Option<String>,
    sticky_l1: bool,
    updated_at: String,
}

pub fn assert_runtime_level_allowed(class: &str, level: &str) -> Result<()> {
    let level = TrustLevel::parse(level)?;
    let cap = hard_cap_for_class(class);
    if level > cap {
        return Err(anyhow!(
            "trust level {} exceeds hard cap {} for action class {}",
            level.as_str(),
            cap.as_str(),
            class
        ));
    }
    Ok(())
}

pub fn hard_cap_for_class(class: &str) -> TrustLevel {
    let clean = class.trim();
    if HARD_CAP_ACTION_CLASSES.contains(&clean) || clean.starts_with(HARD_CAP_TOOL_PREFIX) {
        TrustLevel::L1
    } else if clean == ActionClass::FileWrite.as_str() {
        TrustLevel::L3
    } else {
        // Unknown and not-yet-revertable action classes fail closed. A class
        // is promoted here only after every production adapter has a verified
        // automatic revert path.
        TrustLevel::L1
    }
}

#[allow(dead_code)]
pub fn effective_level_for_action(store: &TaskStore, class: &str) -> Result<TrustLevel> {
    let cap = hard_cap_for_class(class);
    let raw = load_record(store, class)?;
    let level = raw.map(|record| record.level).unwrap_or(TrustLevel::L1);
    Ok(level.min(cap))
}

#[allow(dead_code)]
pub fn level_for_action(store: &TaskStore, class: &str) -> Result<String> {
    Ok(effective_level_for_action(store, class)?
        .as_str()
        .to_string())
}

pub fn set_trust_level(
    store: &TaskStore,
    class: &str,
    level: &str,
    explicit_privacy_center_action: bool,
) -> Result<TrustLevelDto> {
    let class = normalize_class(class);
    let requested = TrustLevel::parse(level)?;
    assert_runtime_level_allowed(&class, requested.as_str())?;
    if requested == TrustLevel::L3 && !explicit_privacy_center_action {
        return Err(anyhow!(
            "L3 trust can only be set by an explicit Privacy Center action"
        ));
    }
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current = load_record_tx(&tx, &class)?;
    let current_level = current
        .as_ref()
        .map(|record| record.level)
        .unwrap_or(TrustLevel::L1)
        .min(hard_cap_for_class(&class));
    let streak = current
        .as_ref()
        .map(|record| record.approval_streak)
        .unwrap_or(0);
    match requested {
        TrustLevel::L1 => {
            upsert_record_tx(&tx, &class, TrustLevel::L1, 0, None, true)?;
        }
        TrustLevel::L2 => {
            if current_level != TrustLevel::L1
                || streak < TRUST_GRADUATION_STREAK
                || current
                    .as_ref()
                    .and_then(|record| record.graduation_offered_at.as_ref())
                    .is_none()
            {
                return Err(anyhow!(
                    "L2 trust requires an earned offer after {} consecutive applied actions",
                    TRUST_GRADUATION_STREAK
                ));
            }
            upsert_record_tx(&tx, &class, TrustLevel::L2, 0, None, false)?;
        }
        TrustLevel::L3 => {
            if current_level != TrustLevel::L2 || streak < TRUST_GRADUATION_STREAK {
                return Err(anyhow!(
                    "L3 trust requires {} successful L2 actions and an explicit Privacy Center action",
                    TRUST_GRADUATION_STREAK
                ));
            }
            upsert_record_tx(&tx, &class, TrustLevel::L3, 0, None, false)?;
        }
    }
    tx.commit()?;
    trust_level_dto(store, &class)
}

pub fn demote_trust_class(store: &TaskStore, class: &str) -> Result<TrustLevelDto> {
    let class = normalize_class(class);
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    upsert_record_tx(&tx, &class, TrustLevel::L1, 0, None, true)?;
    tx.commit()?;
    trust_level_dto(store, &class)
}

pub fn record_receipt_outcome(store: &TaskStore, receipt: &ActionReceiptDto) -> Result<()> {
    let mut conn = store.connect()?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let persisted = tx
        .query_row(
            "SELECT class, status, outcome_accounted_at
             FROM action_receipts WHERE id = ?1",
            params![receipt.id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| anyhow!("action receipt id={} not found", receipt.id))?;
    // idempotency guard: account each applied/rejected outcome once. A revert is
    // a distinct, safety-critical transition (any revert demotes to sticky L1)
    // and is idempotent, so it is always processed even if the receipt was
    // previously accounted as applied.
    if persisted.2.is_some() && persisted.1 != "reverted" {
        return Ok(());
    }
    let class = normalize_class(&persisted.0);
    let current = load_record_tx(&tx, &class)?;
    match persisted.1.as_str() {
        "applied" => {
            let cap = hard_cap_for_class(&class);
            let current_level = current
                .as_ref()
                .map(|record| record.level)
                .unwrap_or(TrustLevel::L1)
                .min(cap);
            let streak = current
                .as_ref()
                .map(|record| record.approval_streak)
                .unwrap_or(0)
                + 1;
            let offer = if streak >= TRUST_GRADUATION_STREAK && current_level < cap {
                current
                    .as_ref()
                    .and_then(|record| record.graduation_offered_at.clone())
                    .or_else(|| Some(now_expr_value()))
            } else {
                current
                    .as_ref()
                    .and_then(|record| record.graduation_offered_at.clone())
            };
            upsert_record_tx(
                &tx,
                &class,
                current_level,
                streak,
                offer,
                current
                    .as_ref()
                    .map(|record| record.sticky_l1)
                    .unwrap_or(false),
            )?;
        }
        "reverted" => {
            upsert_record_tx(&tx, &class, TrustLevel::L1, 0, None, true)?;
        }
        "rejected" => {
            upsert_record_tx(
                &tx,
                &class,
                current
                    .as_ref()
                    .map(|record| record.level)
                    .unwrap_or(TrustLevel::L1)
                    .min(hard_cap_for_class(&class)),
                0,
                None,
                current
                    .as_ref()
                    .map(|record| record.sticky_l1)
                    .unwrap_or(false),
            )?;
        }
        _ => return Ok(()),
    }
    tx.execute(
        "UPDATE action_receipts
         SET outcome_accounted_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = ?1 AND outcome_accounted_at IS NULL",
        params![receipt.id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn list_trust_ladder(store: &TaskStore) -> Result<Vec<TrustLevelDto>> {
    let mut classes = BTreeSet::new();
    for class in DEFAULT_TRUST_CLASSES {
        classes.insert((*class).to_string());
    }
    for class in stored_trust_classes(store)? {
        classes.insert(class);
    }
    for receipt in store.list_action_receipts(None, 200)? {
        classes.insert(receipt.class);
    }

    classes
        .into_iter()
        .map(|class| trust_level_dto(store, &class))
        .collect()
}

#[allow(dead_code)]
pub fn execute_allowed_without_approval(store: &TaskStore, class: &str) -> Result<bool> {
    Ok(effective_level_for_action(store, class)? >= TrustLevel::L2)
}

fn trust_level_dto(store: &TaskStore, class: &str) -> Result<TrustLevelDto> {
    let class = normalize_class(class);
    let record = load_record(store, &class)?;
    let max_level = hard_cap_for_class(&class);
    let level = record
        .as_ref()
        .map(|record| record.level)
        .unwrap_or(TrustLevel::L1)
        .min(max_level);
    let approval_streak = record
        .as_ref()
        .map(|record| record.approval_streak)
        .unwrap_or(0);
    let graduation_offer = if approval_streak >= TRUST_GRADUATION_STREAK
        && level == TrustLevel::L1
        && max_level >= TrustLevel::L2
    {
        Some(format!(
            "Offer L2 for {} after {} consecutive applied actions.",
            class, approval_streak
        ))
    } else if approval_streak >= TRUST_GRADUATION_STREAK
        && level == TrustLevel::L2
        && max_level >= TrustLevel::L3
    {
        Some(format!(
            "{} has {} successful L2 actions; L3 is available only by explicit Privacy Center action.",
            class, approval_streak
        ))
    } else {
        None
    };

    Ok(TrustLevelDto {
        class: class.clone(),
        level: level.as_str().to_string(),
        max_level: max_level.as_str().to_string(),
        approval_streak,
        graduation_offer,
        graduation_offered_at: record
            .as_ref()
            .and_then(|record| record.graduation_offered_at.clone()),
        sticky_l1: record
            .as_ref()
            .map(|record| record.sticky_l1)
            .unwrap_or(false),
        updated_at: record
            .as_ref()
            .map(|record| record.updated_at.clone())
            .unwrap_or_default(),
        recent_history: recent_history(store, &class)?,
    })
}

fn load_record(store: &TaskStore, class: &str) -> Result<Option<TrustLevelRecord>> {
    let conn = store.connect()?;
    conn.query_row(
        "SELECT class, level, approval_streak, graduation_offered_at, sticky_l1, updated_at
         FROM trust_levels WHERE class = ?1",
        params![normalize_class(class)],
        |row| {
            let raw_level: String = row.get(1)?;
            Ok(TrustLevelRecord {
                class: row.get(0)?,
                level: TrustLevel::parse(&raw_level).unwrap_or(TrustLevel::L1),
                approval_streak: row.get(2)?,
                graduation_offered_at: row.get(3)?,
                sticky_l1: row.get::<_, i64>(4)? != 0,
                updated_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map(|record| {
        record.map(|mut record| {
            let cap = hard_cap_for_class(&record.class);
            record.level = record.level.min(cap);
            record
        })
    })
    .map_err(Into::into)
}

fn load_record_tx(tx: &Transaction<'_>, class: &str) -> Result<Option<TrustLevelRecord>> {
    tx.query_row(
        "SELECT class, level, approval_streak, graduation_offered_at, sticky_l1, updated_at
         FROM trust_levels WHERE class = ?1",
        params![normalize_class(class)],
        |row| {
            let raw_level: String = row.get(1)?;
            Ok(TrustLevelRecord {
                class: row.get(0)?,
                level: TrustLevel::parse(&raw_level).unwrap_or(TrustLevel::L1),
                approval_streak: row.get(2)?,
                graduation_offered_at: row.get(3)?,
                sticky_l1: row.get::<_, i64>(4)? != 0,
                updated_at: row.get(5)?,
            })
        },
    )
    .optional()
    .map(|record| {
        record.map(|mut record| {
            record.level = record.level.min(hard_cap_for_class(&record.class));
            record
        })
    })
    .map_err(Into::into)
}

fn upsert_record_tx(
    tx: &Transaction<'_>,
    class: &str,
    level: TrustLevel,
    approval_streak: i64,
    graduation_offered_at: Option<String>,
    sticky_l1: bool,
) -> Result<()> {
    assert_runtime_level_allowed(class, level.as_str())?;
    tx.execute(
        "INSERT INTO trust_levels
         (class, level, approval_streak, graduation_offered_at, sticky_l1, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
         ON CONFLICT(class) DO UPDATE SET
            level = excluded.level,
            approval_streak = excluded.approval_streak,
            graduation_offered_at = excluded.graduation_offered_at,
            sticky_l1 = excluded.sticky_l1,
            updated_at = excluded.updated_at",
        params![
            normalize_class(class),
            level.as_str(),
            approval_streak.max(0),
            graduation_offered_at,
            if sticky_l1 { 1 } else { 0 },
        ],
    )?;
    Ok(())
}

fn stored_trust_classes(store: &TaskStore) -> Result<Vec<String>> {
    let conn = store.connect()?;
    let mut stmt = conn.prepare("SELECT class FROM trust_levels ORDER BY class ASC")?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn recent_history(store: &TaskStore, class: &str) -> Result<Vec<ActionReceiptDto>> {
    Ok(store
        .list_action_receipts(None, 200)?
        .into_iter()
        .filter(|receipt| receipt.class == class)
        .take(5)
        .collect())
}

fn normalize_class(class: &str) -> String {
    let clean = class.trim();
    if clean.starts_with(HARD_CAP_TOOL_PREFIX) {
        clean.to_string()
    } else if clean == "tool.custom.*" {
        clean.to_string()
    } else {
        ActionClass::parse(clean)
            .map(|class| class.as_str())
            .unwrap_or_else(|| clean.to_string())
    }
}

fn now_expr_value() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;
    use crate::action_bus::{revert_action_receipt, FileWriteAdapter, FileWritePayload};

    fn store() -> (TempDir, TaskStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = TaskStore::initialize(dir.path()).unwrap();
        (dir, store)
    }

    // earn L2 legitimately: 10 applied receipts produce a graduation offer, then
    // the explicit set is accepted. L2 is offered, never taken by shortcut.
    fn earn_l2_for(store: &TaskStore, task_id: i64, class: &str) {
        for index in 0..TRUST_GRADUATION_STREAK {
            let receipt = store
                .create_action_receipt(
                    task_id,
                    class,
                    "test",
                    "L1",
                    &format!("earned approval {index}"),
                    "{}",
                    "applied",
                    None,
                    None,
                )
                .unwrap();
            record_receipt_outcome(store, &receipt).unwrap();
        }
        set_trust_level(store, class, "L2", true).unwrap();
    }

    #[test]
    fn d4_hard_caps_clamp_tampered_levels() {
        let (_dir, store) = store();
        let conn = store.connect().unwrap();
        conn.execute(
            "INSERT INTO trust_levels (class, level, approval_streak, sticky_l1)
             VALUES ('email.send', 'L3', 99, 0)",
            [],
        )
        .unwrap();
        assert_eq!(
            effective_level_for_action(&store, "email.send").unwrap(),
            TrustLevel::L1
        );
        assert!(set_trust_level(&store, "file.delete", "L2", true).is_err());
        assert!(set_trust_level(&store, "tool.custom.shell", "L2", true).is_err());
    }

    #[test]
    fn d4_ten_approvals_offer_l2_never_l3() {
        let (_dir, store) = store();
        let task = store.create_task("d4 approvals").unwrap();
        // file.write is the graduatable class (verified auto-revert). Non-
        // revertable classes like doc.replace fail closed at L1.
        for index in 0..TRUST_GRADUATION_STREAK {
            let receipt = store
                .create_action_receipt(
                    task.id,
                    "file.write",
                    "test",
                    "L1",
                    &format!("approval {index}"),
                    "{}",
                    "applied",
                    None,
                    None,
                )
                .unwrap();
            record_receipt_outcome(&store, &receipt).unwrap();
        }
        let ladder = trust_level_dto(&store, "file.write").unwrap();
        assert_eq!(ladder.level, "L1");
        assert_eq!(ladder.max_level, "L3");
        assert_eq!(ladder.approval_streak, TRUST_GRADUATION_STREAK);
        assert!(ladder.graduation_offer.unwrap().contains("Offer L2"));
        // L3 is never offered directly, even after the L2 offer is earned.
        assert!(set_trust_level(&store, "file.write", "L3", false).is_err());
        // a non-revertable class stays capped at L1.
        assert_eq!(hard_cap_for_class("doc.replace"), TrustLevel::L1);
    }

    #[test]
    fn d4_revert_demotes_to_sticky_l1() {
        let (dir, store) = store();
        let task = store.create_task("d4 revert").unwrap();
        earn_l2_for(&store, task.id, "file.write");
        let dest = dir.path().join("trusted.txt");
        fs::write(&dest, "before").unwrap();
        let receipt = FileWriteAdapter::execute_file_write_trusted(
            &store,
            task.id,
            "file",
            "trusted write",
            FileWritePayload {
                allowed_root: dir.path().to_path_buf(),
                destination_path: dest.clone(),
                content: "after".to_string(),
                payload_excerpt: "trusted.txt".to_string(),
            },
        )
        .unwrap();
        revert_action_receipt(&store, receipt.id).unwrap();
        let ladder = trust_level_dto(&store, "file.write").unwrap();
        assert_eq!(ladder.level, "L1");
        assert!(ladder.sticky_l1);
        assert_eq!(fs::read_to_string(dest).unwrap(), "before");
    }

    #[test]
    fn d4_l2_file_write_bus_executes_immediately_with_revert_receipt() {
        let (dir, store) = store();
        let task = store.create_task("d4 l2").unwrap();
        earn_l2_for(&store, task.id, "file.write");
        let dest = dir.path().join("l2.txt");
        let receipt = FileWriteAdapter::execute_file_write_trusted(
            &store,
            task.id,
            "file",
            "l2 trusted write",
            FileWritePayload {
                allowed_root: dir.path().to_path_buf(),
                destination_path: dest.clone(),
                content: "trusted".to_string(),
                payload_excerpt: "l2.txt".to_string(),
            },
        )
        .unwrap();
        assert_eq!(receipt.level, "L2");
        assert_eq!(receipt.status, "applied");
        assert!(receipt.undo_ref.is_some());
        assert_eq!(fs::read_to_string(dest).unwrap(), "trusted");
    }

    #[test]
    fn d4_l1_trusted_file_write_requires_approval() {
        let (dir, store) = store();
        let task = store.create_task("d4 l1").unwrap();
        let err = FileWriteAdapter::execute_file_write_trusted(
            &store,
            task.id,
            "file",
            "l1 trusted write",
            FileWritePayload {
                allowed_root: dir.path().to_path_buf(),
                destination_path: dir.path().join("l1.txt"),
                content: "blocked".to_string(),
                payload_excerpt: "l1.txt".to_string(),
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("requires approval at L1"));
    }
}
