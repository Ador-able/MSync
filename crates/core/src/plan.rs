use crate::snapshot::{same, Item, Snapshot};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conflict {
    pub path: String,
    pub local: Item,
    pub usb: Item,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Copy { from: usize },
    Delete { side: usize },
}
#[derive(Default)]
pub struct Plan {
    pub actions: BTreeMap<String, Action>,
    pub conflicts: Vec<Conflict>,
}
pub fn build(
    current: &[Snapshot; 2],
    previous: Option<&[Snapshot; 2]>,
    precision: i64,
    choices: &BTreeMap<String, usize>,
) -> Result<Plan> {
    let mut plan = Plan::default();
    let mut paths = BTreeSet::new();
    for snapshot in current {
        paths.extend(snapshot.keys().cloned());
    }
    if let Some(old) = previous {
        for snapshot in old {
            paths.extend(snapshot.keys().cloned());
        }
    }
    for path in paths {
        let a = current[0].get(&path);
        let b = current[1].get(&path);
        if let (Some(a), Some(b)) = (a, b) {
            ensure!(
                a.directory == b.directory,
                "同一路径分别是文件和目录：{path}"
            );
        }
        if a.or(b).is_some_and(|i| i.directory) {
            continue;
        }
        if same(a, b) {
            continue;
        }
        let old = previous
            .map(|s| [s[0].get(&path), s[1].get(&path)])
            .unwrap_or([None, None]);
        let changed = [!same(a, old[0]), !same(b, old[1])];
        let action = match (a, b) {
            (Some(a), Some(b)) => {
                if previous.is_some() && changed[0] != changed[1] {
                    Some(Action::Copy {
                        from: if changed[0] { 0 } else { 1 },
                    })
                } else if let Some(&choice) = choices.get(&path) {
                    ensure!(choice < 2, "无效的冲突选项");
                    Some(Action::Copy { from: choice })
                // 首次合并或标记丢失时，不按时间猜测用户要保留的版本。
                } else if previous.is_none()
                    || a.modified == 0
                    || b.modified == 0
                    || (a.modified - b.modified).abs() <= precision
                    || a.modified.max(b.modified) > chrono::Utc::now().timestamp_millis() + 86400000
                {
                    plan.conflicts.push(Conflict {
                        path: path.clone(),
                        local: a.clone(),
                        usb: b.clone(),
                    });
                    None
                } else {
                    Some(Action::Copy {
                        from: if a.modified > b.modified { 0 } else { 1 },
                    })
                }
            }
            (Some(_), None) => {
                if previous.is_some() && old[1].is_some() && !changed[0] {
                    Some(Action::Delete { side: 0 })
                } else {
                    Some(Action::Copy { from: 0 })
                }
            }
            (None, Some(_)) => {
                if previous.is_some() && old[0].is_some() && !changed[1] {
                    Some(Action::Delete { side: 1 })
                } else {
                    Some(Action::Copy { from: 1 })
                }
            }
            _ => None,
        };
        if let Some(action) = action {
            plan.actions.insert(path, action);
        }
    }
    Ok(plan)
}
