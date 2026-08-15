//! Querying the event index.

use protect_api_types::{CameraInfo, EventPage, EventQuery, EventRecord, IndexStats};
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

use crate::upb::reconcile::status_from_str;

/// Page size ceiling. A filter that matches everything should still return a
/// page, not the whole index.
const MAX_LIMIT: i64 = 500;
const DEFAULT_LIMIT: i64 = 100;

/// Build the shared `WHERE` clause for a query.
///
/// Written once and used for both the page and its count, so the total can
/// never disagree with the rows — a mismatch there produces pagination that
/// walks off the end of the results. Columns are qualified with `e.` because
/// the page query joins `cameras`, which also has a `camera_id`; both queries
/// therefore alias `events` as `e`.
fn push_filters(qb: &mut QueryBuilder<Sqlite>, q: &EventQuery) {
    qb.push(" WHERE 1=1");

    if let Some(camera) = &q.camera_id {
        qb.push(" AND e.camera_id = ").push_bind(camera.clone());
    }
    if let Some(t) = &q.event_type {
        qb.push(" AND e.event_type = ").push_bind(t.clone());
    }
    if let Some(sub) = &q.subtype {
        // Subtypes are stored space-padded, so matching " person " cannot also
        // match a longer detection name that merely contains it.
        qb.push(" AND e.subtypes LIKE ")
            .push_bind(format!("% {sub} %"));
    }
    if let Some(status) = q.status {
        qb.push(" AND e.status = ")
            .push_bind(crate::upb::reconcile::status_str(status).to_string());
    }
    if let Some(from) = q.from {
        qb.push(" AND e.start >= ").push_bind(from);
    }
    if let Some(to) = q.to {
        qb.push(" AND e.start <= ").push_bind(to);
    }
}

pub async fn query(pool: &SqlitePool, q: &EventQuery) -> anyhow::Result<EventPage> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let offset = q.offset.unwrap_or(0).max(0);

    let mut count_qb = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM events e");
    push_filters(&mut count_qb, q);
    let total: i64 = count_qb.build_query_scalar().fetch_one(pool).await?;

    let mut qb = QueryBuilder::<Sqlite>::new(
        "SELECT e.id, e.camera_id, e.camera_name, e.event_type, e.subtypes, e.start, e.end,
                e.duration, e.status, e.clip_path, e.size_bytes,
                c.display_name, c.derived_name
           FROM events e
           LEFT JOIN cameras c ON c.camera_id = e.camera_id",
    );
    push_filters(&mut qb, q);
    qb.push(" ORDER BY e.start DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);

    let events = qb
        .build()
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| {
            let camera_id: String = r.get("camera_id");
            EventRecord {
                camera: r
                    .get::<Option<String>, _>("display_name")
                    .or_else(|| r.get::<Option<String>, _>("derived_name"))
                    .or_else(|| r.get::<Option<String>, _>("camera_name"))
                    // Falling back to the raw id keeps a camera visible even
                    // when nothing has ever told us its name.
                    .unwrap_or_else(|| camera_id.clone()),
                id: r.get("id"),
                camera_id,
                event_type: r.get("event_type"),
                subtypes: r
                    .get::<String, _>("subtypes")
                    .split_whitespace()
                    .map(str::to_string)
                    .collect(),
                start: r.get("start"),
                end: r.get("end"),
                duration: r.get("duration"),
                status: status_from_str(&r.get::<String, _>("status")),
                clip_path: r.get("clip_path"),
                size_bytes: r.get("size_bytes"),
            }
        })
        .collect();

    Ok(EventPage { events, total, offset, limit })
}

pub async fn cameras(pool: &SqlitePool) -> anyhow::Result<Vec<CameraInfo>> {
    let rows = sqlx::query(
        "SELECT c.camera_id, c.derived_name, c.display_name,
                COUNT(e.id) AS event_count, MAX(e.start) AS last_event
           FROM cameras c
           LEFT JOIN events e ON e.camera_id = c.camera_id
          GROUP BY c.camera_id
          ORDER BY event_count DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let camera_id: String = r.get("camera_id");
            let derived: Option<String> = r.get("derived_name");
            let display: Option<String> = r.get("display_name");
            CameraInfo {
                display_name: display
                    .clone()
                    .or_else(|| derived.clone())
                    .unwrap_or_else(|| camera_id.clone()),
                camera_id,
                derived_name: derived,
                event_count: r.get("event_count"),
                last_event: r.get("last_event"),
            }
        })
        .collect())
}

pub async fn stats(pool: &SqlitePool) -> anyhow::Result<IndexStats> {
    let counts = sqlx::query(
        "SELECT
            COUNT(*) AS total,
            SUM(status = 'live')             AS live,
            SUM(status = 'archived')         AS archived,
            SUM(status = 'vanished')         AS vanished,
            SUM(status = 'pending_backfill') AS pending,
            SUM(status = 'never_backed_up')  AS never,
            MAX(start) AS newest,
            MIN(start) AS oldest,
            -- The newest event that actually produced a clip. Comparing this
            -- to now is what detects a backup service that is running but no
            -- longer capturing anything.
            MAX(CASE WHEN clip_path IS NOT NULL THEN start END) AS newest_captured
         FROM events",
    )
    .fetch_one(pool)
    .await?;

    let newest_captured: Option<f64> = counts.get("newest_captured");
    let state = sqlx::query("SELECT last_sync, last_error FROM index_state WHERE id = 1")
        .fetch_optional(pool)
        .await?;

    // Distinct detection types actually present, so the filter UI offers what
    // exists rather than a hardcoded list that would be wrong on any setup
    // with different cameras.
    let subtype_rows = sqlx::query("SELECT DISTINCT subtypes FROM events WHERE subtypes != ' '")
        .fetch_all(pool)
        .await?;
    let mut subs: Vec<String> = subtype_rows
        .iter()
        .flat_map(|r| {
            r.get::<String, _>("subtypes")
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    subs.sort();
    subs.dedup();

    Ok(IndexStats {
        total_events: counts.get::<i64, _>("total"),
        live_clips: counts.try_get::<i64, _>("live").unwrap_or(0),
        archived: counts.try_get::<i64, _>("archived").unwrap_or(0),
        vanished: counts.try_get::<i64, _>("vanished").unwrap_or(0),
        pending_backfill: counts.try_get::<i64, _>("pending").unwrap_or(0),
        never_backed_up: counts.try_get::<i64, _>("never").unwrap_or(0),
        backup_lag_secs: newest_captured
            .map(|t| (crate::upb::reconcile::now_secs() - t).max(0.0)),
        newest_event: counts.get("newest"),
        oldest_event: counts.get("oldest"),
        last_sync: state.as_ref().and_then(|r| r.get("last_sync")),
        last_sync_error: state.as_ref().and_then(|r| r.get("last_error")),
        distinct_subtypes: subs,
    })
}

/// Event types present in the index, for the filter UI.
pub async fn event_types(pool: &SqlitePool) -> anyhow::Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT DISTINCT event_type FROM events ORDER BY event_type")
            .fetch_all(pool)
            .await?,
    )
}
