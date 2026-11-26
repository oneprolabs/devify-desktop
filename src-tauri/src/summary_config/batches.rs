//! Plan and resume budgeted local summaries without omitting any selected messages.
use super::prompt::{finish_local_prompt, local_material, render_local_prompt, Frozen};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

const MAX_BATCH_CHARACTERS: usize = 40_000;
const MAX_HISTORY_BYTES: usize = 4 * 1024 * 1024;
const BATCH_LIMIT_ERROR: &str = "单条消息超过本地单次总结范围，无法完整生成。";
const MERGE_LIMIT_ERROR: &str = "总结规则或我的要求占用过多输入空间，无法合并分批摘要。";

#[derive(Debug)]
pub(crate) struct Plan {
    pub ends: Vec<usize>,
    pub bytes: usize,
}

impl Plan {
    pub(super) fn build(snapshot: &Frozen, body: &Value) -> Result<Self, String> {
        let messages = messages(body)?;
        if serde_json::to_vec(messages)
            .map_err(|e| e.to_string())?
            .len()
            > MAX_HISTORY_BYTES
        {
            return Err("本次聊天记录超过本地任务的 4 MiB 上限，请缩小范围。".into());
        }
        if messages
            .windows(2)
            .any(|pair| pair[0]["time"].as_i64() > pair[1]["time"].as_i64())
        {
            return Err("总结消息顺序已变化，请重新读取。".into());
        }

        let counts: Vec<_> = messages
            .iter()
            .map(|item| item["text"].as_str().unwrap_or_default().chars().count())
            .collect();
        if counts.iter().copied().sum::<usize>() <= MAX_BATCH_CHARACTERS {
            if let Some(single) = batch_prompt(snapshot, body, messages, "")? {
                return Ok(Self {
                    ends: vec![messages.len()],
                    bytes: single.len(),
                });
            }
        }

        let output_limit = draft_budget(snapshot, body)?;
        let note = draft_note(output_limit);
        let mut ends = Vec::new();
        let mut bytes = 0usize;
        let mut start = 0;
        while start < messages.len() {
            let mut characters = 0;
            // Even the shortest compact row takes more than 16 bytes.
            let max_rows = snapshot.scenario.budgets.local_cli.max_request_bytes / 16;
            let candidates = counts[start..]
                .iter()
                .take(max_rows)
                .take_while(|count| {
                    characters += **count;
                    characters <= MAX_BATCH_CHARACTERS
                })
                .count();
            let (mut low, mut high) = (1, candidates);
            let mut best = None;
            while low <= high {
                let size = low + (high - low) / 2;
                match batch_prompt(snapshot, body, &messages[start..start + size], &note)? {
                    Some(prompt) => {
                        best = Some((size, prompt.len()));
                        low = size + 1;
                    }
                    None => high = size - 1,
                }
            }
            let (size, estimated_bytes) = best.ok_or(BATCH_LIMIT_ERROR)?;
            start += size;
            ends.push(start);
            bytes += estimated_bytes;
        }
        Ok(Self { ends, bytes })
    }
}

fn messages(body: &Value) -> Result<&[Value], String> {
    body["messages"]
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "缺少总结消息。".into())
}

fn batch_prompt(
    snapshot: &Frozen,
    body: &Value,
    messages: &[Value],
    note: &str,
) -> Result<Option<String>, String> {
    let characters = messages
        .iter()
        .filter_map(|item| item["text"].as_str())
        .map(|text| text.chars().count())
        .sum();
    let sessions = messages
        .iter()
        .filter_map(|item| item["username"].as_str())
        .collect::<HashSet<_>>()
        .len();
    render_local_prompt(snapshot, body, messages, characters, sessions, note)
}

fn draft_note(max_bytes: usize) -> String {
    format!("这是整个历史的一部分。生成用于后续汇总的简明摘要，保留日期、人物、决定、待办及可回查的线索。请控制在约 {} 字以内。\n", max_bytes / 4)
}

fn merge_prompt(
    snapshot: &Frozen,
    body: &Value,
    drafts: &[String],
    limit: Option<usize>,
) -> Result<Option<String>, String> {
    let mut material = local_material(body)?;
    material["ordered_summaries"] = json!(drafts);
    let note = match limit {
        Some(bytes) => draft_note(bytes),
        None => "将按时间排序的各段摘要合成一份最终总结。去重并串联跨段事件，保留日期、人物、决定、待办和线索。不要凭空补充。\n".into(),
    };
    finish_local_prompt(snapshot, &note, &material)
}

fn draft_budget(snapshot: &Frozen, body: &Value) -> Result<usize, String> {
    let empty = merge_prompt(snapshot, body, &[], Some(0))?.ok_or(MERGE_LIMIT_ERROR)?;
    let final_empty = merge_prompt(snapshot, body, &[], None)?.ok_or(MERGE_LIMIT_ERROR)?;
    // Allow two JSON-escaped drafts and the separators to fit even on the last round.
    let room = snapshot
        .scenario
        .budgets
        .local_cli
        .max_request_bytes
        .saturating_sub(empty.len().max(final_empty.len()).saturating_add(16));
    let limit = room / 3;
    if limit < 256 {
        return Err(MERGE_LIMIT_ERROR.into());
    }
    Ok(limit)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Progress {
    ends: Vec<usize>,
    completed: usize,
    drafts: Vec<String>,
    merged: Vec<String>,
    cursor: usize,
}

pub(crate) enum Step {
    Batch(String),
    Merge { prompt: String, end: usize },
    Final(String),
}

impl Progress {
    pub(crate) fn new(plan: Plan) -> Self {
        Self {
            ends: plan.ends,
            completed: 0,
            drafts: Vec::new(),
            merged: Vec::new(),
            cursor: 0,
        }
    }

    pub(crate) fn counts(&self) -> (usize, usize) {
        (self.completed, self.ends.len())
    }

    pub(crate) fn next(&mut self, snapshot: &Frozen, body: &Value) -> Result<Step, String> {
        let limit = draft_budget(snapshot, body)?;
        if let Some(&end) = self.ends.get(self.completed) {
            let start = self.completed.checked_sub(1).map_or(0, |i| self.ends[i]);
            let slice = messages(body)?
                .get(start..end)
                .ok_or("分批总结的消息范围无效。")?;
            return Ok(Step::Batch(
                batch_prompt(snapshot, body, slice, &draft_note(limit))?
                    .ok_or(BATCH_LIMIT_ERROR)?,
            ));
        }

        if self.cursor + 1 == self.drafts.len() && self.cursor > 0 {
            self.merged.push(self.drafts[self.cursor].clone());
            self.cursor += 1;
        }
        if self.cursor == self.drafts.len() {
            self.drafts = std::mem::take(&mut self.merged);
            self.cursor = 0;
        }
        let remaining = &self.drafts[self.cursor..];
        if self.cursor == 0 {
            if let Some(prompt) = merge_prompt(snapshot, body, remaining, None)? {
                return Ok(Step::Final(prompt));
            }
        }
        let mut best = None;
        for count in 2..=remaining.len() {
            let Some(prompt) = merge_prompt(snapshot, body, &remaining[..count], Some(limit))?
            else {
                break;
            };
            best = Some(Step::Merge {
                prompt,
                end: self.cursor + count,
            });
        }
        best.ok_or_else(|| MERGE_LIMIT_ERROR.into())
    }

    pub(crate) fn accept(
        &mut self,
        snapshot: &Frozen,
        body: &Value,
        step: Step,
        output: String,
    ) -> Result<Option<String>, String> {
        if matches!(step, Step::Final(_)) {
            return Ok(Some(output));
        }
        let encoded = serde_json::to_vec(&output).map_err(|e| e.to_string())?;
        if output.trim().is_empty() || encoded.len() > draft_budget(snapshot, body)? {
            return Err("本地工具返回的分批摘要为空或过长，请重试。".into());
        }
        match step {
            Step::Batch(_) => {
                self.drafts.push(output);
                self.completed += 1;
            }
            Step::Merge { end, .. } => {
                self.merged.push(output);
                self.cursor = end;
            }
            Step::Final(_) => unreachable!(),
        }
        Ok(None)
    }
}

impl Step {
    pub(crate) fn prompt(&self) -> &str {
        match self {
            Self::Batch(prompt) | Self::Merge { prompt, .. } | Self::Final(prompt) => prompt,
        }
    }
}
