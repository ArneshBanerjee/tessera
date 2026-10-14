//! Sprint reports, rebuilt from issue history.
//!
//! An issue's history records every change to its sprint, its estimate, and its status. From
//! it, a [`Timeline`] answers what the issue looked like at any moment, and the reports add
//! those answers up: what a sprint committed to, what it completed, what came and went during
//! it, and how much work remained over time.

use sqlx::SqlitePool;

use crate::store::changelog;
use crate::store::issues::{self, Issue};
use crate::store::meta::Catalog;
use crate::store::sprints::{self, Sprint};
use crate::time::now_ms;

/// The name the history uses for the estimate field.
const ESTIMATE_FIELD: &str = "Story point estimate";

/// One change, at one moment.
enum Change {
    Sprint { from: Option<i64>, to: Option<i64> },
    Estimate { from: Option<f64>, to: Option<f64> },
    Status { from: Option<i64>, to: Option<i64> },
}

/// What an issue looked like over time.
pub struct Timeline {
    pub issue: Issue,
    member_now: bool,
    estimate_now: Option<f64>,
    events: Vec<(i64, Change)>,
}

fn number(text: &Option<String>) -> Option<f64> {
    text.as_deref().and_then(|t| t.trim().parse().ok())
}

fn id(text: &Option<String>) -> Option<i64> {
    text.as_deref().and_then(|t| t.parse().ok())
}

impl Timeline {
    pub async fn load(
        db: &SqlitePool,
        issue: Issue,
        sprint_id: i64,
        estimate_field: Option<i64>,
    ) -> sqlx::Result<Timeline> {
        let estimate_now = match estimate_field {
            Some(field) => issues::custom_values(db, issue.id)
                .await?
                .into_iter()
                .find(|(f, _)| *f == field)
                .and_then(|(_, value)| serde_json::from_str::<serde_json::Value>(&value).ok())
                .and_then(|v| v.as_f64()),
            None => None,
        };
        let mut events = Vec::new();
        for entry in changelog::for_issue(db, issue.id).await? {
            for item in entry.items {
                let change = match item.field.as_str() {
                    "Sprint" => Change::Sprint {
                        from: id(&item.from_value),
                        to: id(&item.to_value),
                    },
                    ESTIMATE_FIELD => Change::Estimate {
                        from: number(&item.from_string),
                        to: number(&item.to_string),
                    },
                    "status" => Change::Status {
                        from: id(&item.from_value),
                        to: id(&item.to_value),
                    },
                    _ => continue,
                };
                events.push((entry.created_at, change));
            }
        }
        Ok(Timeline {
            member_now: sprints::is_member(db, sprint_id, issue.id).await?,
            issue,
            estimate_now,
            events,
        })
    }

    /// Whether the issue was in the sprint at `at`.
    pub fn in_sprint(&self, sprint_id: i64, at: i64) -> bool {
        if at < self.issue.created_at {
            return false;
        }
        let moves: Vec<(i64, Option<i64>, Option<i64>)> = self
            .events
            .iter()
            .filter_map(|(t, c)| match c {
                Change::Sprint { from, to } => Some((*t, *from, *to)),
                _ => None,
            })
            .collect();
        match moves.iter().rev().find(|(t, _, _)| *t <= at) {
            Some((_, _, to)) => *to == Some(sprint_id),
            // Before its first move, the issue was where that move took it from.
            None => match moves.first() {
                Some((_, from, _)) => *from == Some(sprint_id),
                None => self.member_now,
            },
        }
    }

    /// The estimate at `at`.
    pub fn estimate(&self, at: i64) -> Option<f64> {
        let changes: Vec<(i64, Option<f64>, Option<f64>)> = self
            .events
            .iter()
            .filter_map(|(t, c)| match c {
                Change::Estimate { from, to } => Some((*t, *from, *to)),
                _ => None,
            })
            .collect();
        match changes.iter().rev().find(|(t, _, _)| *t <= at) {
            Some((_, _, to)) => *to,
            None => changes
                .first()
                .map_or(self.estimate_now, |(_, from, _)| *from),
        }
    }

    /// The status at `at`.
    pub fn status(&self, at: i64) -> i64 {
        let changes: Vec<(i64, Option<i64>, Option<i64>)> = self
            .events
            .iter()
            .filter_map(|(t, c)| match c {
                Change::Status { from, to } => Some((*t, *from, *to)),
                _ => None,
            })
            .collect();
        let found = match changes.iter().rev().find(|(t, _, _)| *t <= at) {
            Some((_, _, to)) => *to,
            None => changes.first().and_then(|(_, from, _)| *from),
        };
        found.unwrap_or(self.issue.status_id)
    }

    /// Moments when something about the issue changed.
    fn moments(&self) -> impl Iterator<Item = i64> + '_ {
        self.events.iter().map(|(t, _)| *t)
    }
}

/// The start of a sprint for reports: when it was started, or its planned start.
pub fn sprint_start(sprint: &Sprint) -> i64 {
    sprint
        .activated_at
        .or(sprint.start_date)
        .unwrap_or_else(now_ms)
}

/// The end of a sprint for reports: when it was completed, or now.
pub fn sprint_end(sprint: &Sprint) -> i64 {
    sprint.complete_date.unwrap_or_else(now_ms)
}

/// Every issue that was ever in the sprint, with its history.
pub async fn timelines(
    db: &SqlitePool,
    sprint: &Sprint,
    estimate_field: Option<i64>,
) -> sqlx::Result<Vec<Timeline>> {
    let mut out = Vec::new();
    for id in sprints::issue_ids_ever_in(db, sprint.id).await? {
        if let Some(issue) = issues::by_id(db, id).await? {
            out.push(Timeline::load(db, issue, sprint.id, estimate_field).await?);
        }
    }
    Ok(out)
}

/// How one issue fared in a sprint.
pub struct Outcome<'a> {
    pub timeline: &'a Timeline,
    /// The estimate when the sprint started, or when the issue joined it.
    pub initial_estimate: Option<f64>,
    /// The estimate when the sprint ended.
    pub final_estimate: Option<f64>,
    pub added_during_sprint: bool,
}

/// The sprint report's sorting of issues.
#[derive(Default)]
pub struct SprintReport<'a> {
    pub completed: Vec<Outcome<'a>>,
    pub not_completed: Vec<Outcome<'a>>,
    /// Removed from the sprint before it ended.
    pub punted: Vec<Outcome<'a>>,
}

pub fn sprint_report<'a>(
    sprint: &Sprint,
    catalog: &Catalog,
    timelines: &'a [Timeline],
) -> SprintReport<'a> {
    let start = sprint_start(sprint);
    let end = sprint_end(sprint);
    let mut report = SprintReport::default();
    for timeline in timelines {
        let at_start = timeline.in_sprint(sprint.id, start);
        let joined = timeline
            .moments()
            .filter(|t| *t > start && *t <= end)
            .find(|t| timeline.in_sprint(sprint.id, *t));
        if !at_start && joined.is_none() {
            continue;
        }
        let outcome = Outcome {
            timeline,
            initial_estimate: timeline.estimate(if at_start {
                start
            } else {
                joined.unwrap_or(start)
            }),
            final_estimate: timeline.estimate(end),
            added_during_sprint: !at_start,
        };
        if !timeline.in_sprint(sprint.id, end) {
            report.punted.push(outcome);
        } else if catalog.is_done(timeline.status(end)) {
            report.completed.push(outcome);
        } else {
            report.not_completed.push(outcome);
        }
    }
    report
}

/// The work left in the sprint at one moment.
pub fn remaining(sprint_id: i64, catalog: &Catalog, timelines: &[Timeline], at: i64) -> f64 {
    timelines
        .iter()
        .filter(|t| t.in_sprint(sprint_id, at) && !catalog.is_done(t.status(at)))
        .filter_map(|t| t.estimate(at))
        .sum()
}

/// The burndown: the work left at the start, then after each change during the sprint.
pub fn burndown(sprint: &Sprint, catalog: &Catalog, timelines: &[Timeline]) -> Vec<(i64, f64)> {
    let start = sprint_start(sprint);
    let end = sprint_end(sprint);
    let mut moments: Vec<i64> = timelines
        .iter()
        .flat_map(|t| t.moments())
        .filter(|t| *t > start && *t <= end)
        .collect();
    moments.sort_unstable();
    moments.dedup();
    let mut points = vec![(start, remaining(sprint.id, catalog, timelines, start))];
    for at in moments {
        let value = remaining(sprint.id, catalog, timelines, at);
        if points.last().is_none_or(|(_, last)| *last != value) {
            points.push((at, value));
        }
    }
    let last = points.last().map_or(0.0, |(_, v)| *v);
    points.push((end, last));
    points
}

/// Story points committed at the start, and completed by the end.
pub fn velocity(sprint: &Sprint, catalog: &Catalog, timelines: &[Timeline]) -> (f64, f64) {
    let start = sprint_start(sprint);
    let committed = timelines
        .iter()
        .filter(|t| t.in_sprint(sprint.id, start))
        .filter_map(|t| t.estimate(start))
        .sum();
    let report = sprint_report(sprint, catalog, timelines);
    let completed = report
        .completed
        .iter()
        .filter_map(|o| o.final_estimate)
        .sum();
    (committed, completed)
}
