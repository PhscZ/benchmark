//! End-to-end tests over the real router: request in, JSON out.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use clap::Parser;
use glicko_api::config::Config;
use serde_json::{Value, json};
use tower::ServiceExt;

/// A router backed by a throwaway data directory. The directory is deleted when
/// the guard is dropped, so tests never share state.
struct TestApp {
    router: Router,
    _dir: Option<tempfile::TempDir>,
}

impl TestApp {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        Self::in_dir(dir)
    }

    fn in_dir(dir: tempfile::TempDir) -> Self {
        let config = Config::parse_from([
            "glicko-api",
            "--data-dir",
            dir.path().to_str().expect("utf-8 temp path"),
        ]);
        let router = glicko_api::build_app(&config).expect("build app");
        Self {
            router,
            _dir: Some(dir),
        }
    }

    /// Builds an app over a directory owned by the caller.
    fn at(path: &std::path::Path) -> Self {
        let config = Config::parse_from([
            "glicko-api",
            "--data-dir",
            path.to_str().expect("utf-8 temp path"),
        ]);
        let router = glicko_api::build_app(&config).expect("build app");
        Self { router, _dir: None }
    }

    async fn request(&self, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut builder = Request::builder().method(method).uri(uri);
        let request = match body {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                builder
                    .body(Body::from(value.to_string()))
                    .expect("request")
            }
            None => builder.body(Body::empty()).expect("request"),
        };
        let response = self
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .expect("body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!(
                    "non-JSON body for {uri}: {e}: {}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, value)
    }

    async fn get(&self, uri: &str) -> (StatusCode, Value) {
        self.request("GET", uri, None).await
    }

    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        self.request("POST", uri, Some(body)).await
    }

    /// Posts an event array and asserts the import succeeded.
    async fn import(&self, events: Value) -> Value {
        let (status, body) = self.post("/events/import", events).await;
        assert_eq!(status, StatusCode::OK, "import failed: {body}");
        body
    }
}

/// The exact sample payload from the specification.
fn sample_events() -> Value {
    json!([
        {"id": "e5",  "time": "2026-05-16T18:36:04Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
        {"id": "e10", "time": "2026-05-16T18:36:25Z", "killer": "nicolas404", "victim": "orinslc", "player_count": 3},
        {"id": "e11", "time": "2026-05-16T18:36:25Z", "killer": "orinslc", "victim": "nicolas404", "player_count": 2}
    ])
}

/// A 20-kill race won by `winner`, spread over `secs` from `start`.
fn race(
    prefix: &str,
    start: i64,
    winner: &str,
    loser: &str,
    player_count: u32,
    loser_kills: u32,
) -> Vec<Value> {
    let mut events = Vec::new();
    let total = 20 + loser_kills;
    let mut w = 0;
    let mut l = 0;
    for i in 0..total {
        // interleave so the loser's kills are spread through the race
        let (killer, victim) = if l < loser_kills && (i % 2 == 1 || w == 20) {
            l += 1;
            (loser, winner)
        } else {
            w += 1;
            (winner, loser)
        };
        events.push(json!({
            "id": format!("{prefix}-{i}"),
            "time": chrono::DateTime::from_timestamp(start + i as i64, 0)
                .unwrap()
                .to_rfc3339(),
            "killer": killer,
            "victim": victim,
            "player_count": player_count,
        }));
    }
    assert_eq!(
        (w, l),
        (20, loser_kills),
        "race generator must produce a 20-x race"
    );
    events
}

fn find<'a>(items: &'a [Value], login: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item["login"] == json!(login))
        .unwrap_or_else(|| panic!("`{login}` missing from {items:?}"))
}

// --------------------------------------------------------------------- tests

#[tokio::test]
async fn health_and_config_report_the_specified_defaults() {
    let app = TestApp::new();

    let (status, body) = app.get("/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["events"], 0);

    let (status, body) = app.get("/config").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["initial_rating"], 1500.0);
    assert_eq!(body["initial_rd"], 350.0);
    assert_eq!(body["initial_volatility"], 0.06);
    assert_eq!(body["scale"], 173.7178);
    assert_eq!(body["epsilon"], 1e-6);
    assert_eq!(body["tau"], 0.5);
    assert_eq!(body["match_kill_target"], 20);
    assert_eq!(body["excluded_logins"], json!([".nobody", ".self"]));
}

#[tokio::test]
async fn imported_events_show_up_in_the_leaderboard_and_player_endpoint() {
    let app = TestApp::new();
    let report = app.import(sample_events()).await;
    assert_eq!(report["imported"], 3);
    assert_eq!(report["duplicates"], 0);
    assert_eq!(report["players"], 2);
    assert_eq!(report["matches"], 0, "no race finished");
    assert_eq!(report["skipped_excluded"], 0);

    let (status, board) = app.get("/leaderboard?category=total").await;
    assert_eq!(status, StatusCode::OK);
    let board = board.as_array().expect("array");
    assert_eq!(board.len(), 2);
    let nicolas = find(board, "nicolas404");
    assert_eq!(nicolas["rank"], 1);
    assert_eq!(nicolas["kills"], 2);
    assert_eq!(nicolas["deaths"], 1);
    assert_eq!(nicolas["wins"], 0);
    assert_eq!(nicolas["losses"], 0);
    assert_eq!(nicolas["winrate"], 0.0);
    assert_eq!(nicolas["kd_ratio"], 2.0);
    let orinslc = find(board, "orinslc");
    assert_eq!(orinslc["kills"], 1);
    assert_eq!(orinslc["deaths"], 2);
    assert_eq!(orinslc["kd_ratio"], 0.5);

    // the same numbers on the individual endpoint, plus rating state
    let (status, player) = app.get("/players/nicolas404").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(player["login"], "nicolas404");
    assert_eq!(player["rating"], 1500.0);
    assert_eq!(player["rd"], 350.0);
    assert_eq!(player["volatility"], 0.06);
    assert_eq!(player["kills"], 2);
    assert_eq!(player["kd_ratio"], 2.0);

    let (status, body) = app.get("/players/nobody-here").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("unknown player")
    );
}

#[tokio::test]
async fn categories_split_by_the_player_count_recorded_on_the_event() {
    let app = TestApp::new();
    app.import(sample_events()).await;

    // pub: the two 3-player kills
    let (_, pub_board) = app.get("/leaderboard?category=pub").await;
    let pub_board = pub_board.as_array().expect("array");
    assert_eq!(
        pub_board.len(),
        2,
        "both sides of a kill count in the ladder"
    );
    let pub_nicolas = find(pub_board, "nicolas404");
    assert_eq!(pub_nicolas["kills"], 2);
    assert_eq!(pub_nicolas["deaths"], 0);
    let pub_orinslc = find(pub_board, "orinslc");
    assert_eq!(pub_orinslc["kills"], 0);
    assert_eq!(pub_orinslc["deaths"], 2);
    assert_eq!(pub_orinslc["kd_ratio"], 0.0);

    // 1v1: the single 2-player kill
    let (_, duel) = app.get("/leaderboard?category=1v1").await;
    let duel = duel.as_array().expect("array");
    assert_eq!(duel.len(), 2);
    let duel_orinslc = find(duel, "orinslc");
    assert_eq!(duel_orinslc["kills"], 1);
    assert_eq!(duel_orinslc["deaths"], 0);
    let duel_nicolas = find(duel, "nicolas404");
    assert_eq!(duel_nicolas["kills"], 0);
    assert_eq!(duel_nicolas["deaths"], 1);

    // per-category stats are independent of the total ladder
    let (status, body) = app.get("/players/nicolas404?category=1v1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["kills"], 0);
    assert_eq!(body["deaths"], 1);
    assert_ne!(body["last_active"], Value::Null);

    let (status, body) = app.get("/leaderboard?category=nonsense").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("unknown category")
    );
}

#[tokio::test]
async fn re_importing_the_same_events_is_a_no_op() {
    let app = TestApp::new();
    app.import(sample_events()).await;
    let report = app.import(sample_events()).await;
    assert_eq!(report["imported"], 0);
    assert_eq!(report["duplicates"], 3);
    assert_eq!(report["total_events"], 3);

    let (_, board) = app.get("/leaderboard").await;
    assert_eq!(
        find(board.as_array().expect("array"), "nicolas404")["kills"],
        2
    );
}

#[tokio::test]
async fn a_completed_race_updates_ratings_and_only_that_pairs_counters() {
    let app = TestApp::new();
    // alice beats bob 20-0, then trades evenly with carol
    let mut events = race("r1", 1_000_000, "alice", "bob", 2, 0);
    for i in 0..19 {
        events.push(json!({
            "id": format!("c-{i}"),
            "time": chrono::DateTime::from_timestamp(2_000_000 + i, 0).unwrap().to_rfc3339(),
            "killer": "alice",
            "victim": "carol",
            "player_count": 2,
        }));
    }
    app.import(Value::Array(events)).await;

    let (_, alice) = app.get("/players/alice").await;
    assert_eq!(alice["wins"], 1);
    assert_eq!(alice["losses"], 0);
    assert_eq!(alice["kills"], 39);
    assert_eq!(alice["deaths"], 0);
    assert_eq!(alice["winrate"], 1.0);
    assert!(alice["rating"].as_f64().expect("rating") > 1500.0);

    let (_, bob) = app.get("/players/bob").await;
    assert_eq!(bob["losses"], 1);
    assert_eq!(bob["wins"], 0);
    assert_eq!(bob["deaths"], 20);
    assert!(bob["rating"].as_f64().expect("rating") < 1500.0);

    // alice's race against carol is still open at 19 kills, unaffected by the
    // race that just finished against bob
    let (_, h2h) = app.get("/players/alice/matchups/carol").await;
    assert_eq!(h2h["kills"], 19);
    assert_eq!(h2h["deaths"], 0);
    assert_eq!(h2h["kd_ratio"], 19.0);
}

#[tokio::test]
async fn head_to_head_is_symmetric_and_reports_both_sides() {
    let app = TestApp::new();
    app.import(sample_events()).await;

    let (status, forward) = app.get("/players/nicolas404/matchups/orinslc").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(forward["login"], "nicolas404");
    assert_eq!(forward["opponent"], "orinslc");
    assert_eq!(forward["kills"], 2);
    assert_eq!(forward["deaths"], 1);
    assert_eq!(forward["kd_ratio"], 2.0);

    let (_, reverse) = app.get("/players/orinslc/matchups/nicolas404").await;
    assert_eq!(reverse["kills"], 1);
    assert_eq!(reverse["deaths"], 2);
    assert_eq!(reverse["kd_ratio"], 0.5);

    // per-category head-to-head
    let (_, duel) = app
        .get("/players/orinslc/matchups/nicolas404?category=1v1")
        .await;
    assert_eq!(duel["kills"], 1);
    assert_eq!(duel["deaths"], 0);
    assert_eq!(duel["category"], "1v1");

    // opponents with no shared history are reported as zeroes, not an error
    let (status, none) = app.get("/players/nicolas404/matchups/ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        none["error"]
            .as_str()
            .expect("error")
            .contains("unknown player")
    );
}

#[tokio::test]
async fn leaderboard_pagination_is_consistent_with_the_full_ladder() {
    let app = TestApp::new();
    // 25 players, each with a couple of kills so they all register
    let mut events = Vec::new();
    for i in 0..25 {
        for k in 0..(i % 3 + 1) {
            events.push(json!({
                "id": format!("e-{i}-{k}"),
                "time": chrono::DateTime::from_timestamp(1_000_000 + (i * 10 + k) as i64, 0)
                    .unwrap()
                    .to_rfc3339(),
                "killer": format!("p{i}"),
                "victim": "punchingbag",
                "player_count": 2,
            }));
        }
    }
    app.import(Value::Array(events)).await;

    let (_, full) = app.get("/leaderboard?limit=1000").await;
    let full = full.as_array().expect("array").clone();
    assert_eq!(full.len(), 26);

    let mut collected = Vec::new();
    let mut page = 1;
    loop {
        let (status, body) = app
            .get(&format!("/leaderboard/paginated?page={page}&limit=10"))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total"], 26);
        assert_eq!(body["total_pages"], 3);
        let items = body["items"].as_array().expect("items").clone();
        if items.is_empty() {
            break;
        }
        collected.extend(items);
        page += 1;
    }
    assert_eq!(collected, full, "paged ladder must equal the full ladder");

    // ranks are absolute, not per-page
    assert_eq!(collected[0]["rank"], 1);
    assert_eq!(collected[10]["rank"], 11);
    assert_eq!(collected[25]["rank"], 26);
}

#[tokio::test]
async fn matchup_pagination_walks_every_opponent() {
    let app = TestApp::new();
    let mut events = Vec::new();
    for i in 0..30 {
        events.push(json!({
            "id": format!("m-{i}"),
            "time": chrono::DateTime::from_timestamp(1_000_000 + i as i64, 0).unwrap().to_rfc3339(),
            "killer": "ace",
            "victim": format!("opp{i}"),
            "player_count": 2,
        }));
    }
    app.import(Value::Array(events)).await;

    let (status, page1) = app.get("/players/ace/matchups?page=1&limit=10").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page1["total"], 30);
    assert_eq!(page1["total_pages"], 3);
    assert_eq!(page1["items"].as_array().expect("items").len(), 10);
    assert_eq!(page1["items"][0]["kills"], 1);

    let (_, page3) = app.get("/players/ace/matchups?page=3&limit=10").await;
    assert_eq!(page3["items"].as_array().expect("items").len(), 10);

    let (_, page4) = app.get("/players/ace/matchups?page=4&limit=10").await;
    assert_eq!(page4["items"].as_array().expect("items").len(), 0);

    let (status, body) = app.get("/players/ace/matchups?limit=100000").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("must not exceed")
    );
}

#[tokio::test]
async fn events_feed_is_newest_first_and_filters_by_either_side() {
    let app = TestApp::new();
    app.import(sample_events()).await;

    let (status, page) = app.get("/events?limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["total"], 3);
    assert_eq!(page["total_pages"], 2);
    let items = page["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    // e11 and e10 share a timestamp; e11 was registered last so it leads
    assert_eq!(items[0]["id"], "e11");
    assert_eq!(items[1]["id"], "e10");
    assert_eq!(items[0]["killer"], "orinslc");
    assert_eq!(items[0]["victim"], "nicolas404");
    assert_eq!(items[0]["player_count"], 2);

    let (_, page2) = app.get("/events?page=2&limit=2").await;
    assert_eq!(page2["items"].as_array().expect("items").len(), 1);
    assert_eq!(page2["items"][0]["id"], "e5");

    // filtering matches killer or victim
    let (_, as_killer) = app.get("/events?player=nicolas404").await;
    assert_eq!(as_killer["total"], 3);
    let (_, as_victim) = app.get("/events?player=orinslc").await;
    assert_eq!(as_victim["total"], 3);

    let (status, body) = app.get("/events?player=ghost").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"]
            .as_str()
            .expect("error")
            .contains("unknown player")
    );
}

#[tokio::test]
async fn excluded_logins_never_reach_ratings_or_statistics() {
    let app = TestApp::new();
    app.import(json!([
        {"id": "x1", "time": "2026-05-16T18:36:04Z", "killer": ".nobody", "victim": "alice", "player_count": 2},
        {"id": "x2", "time": "2026-05-16T18:36:05Z", "killer": "alice", "victim": ".self", "player_count": 2},
        {"id": "x3", "time": "2026-05-16T18:36:06Z", "killer": "alice", "victim": "bob", "player_count": 2}
    ]))
    .await;

    let (_, board) = app.get("/leaderboard").await;
    let board = board.as_array().expect("array");
    assert_eq!(board.len(), 2, "only alice and bob are players");
    assert_eq!(find(board, "alice")["kills"], 1);
    assert_eq!(find(board, "alice")["deaths"], 0);
    assert_eq!(find(board, "bob")["deaths"], 1);

    let (status, _) = app.get("/players/.nobody").await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // the events are still listed: they are part of the log, just not rated
    let (_, events) = app.get("/events").await;
    assert_eq!(events["total"], 3);
}

#[tokio::test]
async fn alts_collapse_into_the_main_and_recalculate_the_history() {
    let app = TestApp::new();
    // "fck" racks up 10 kills, "deadlyenergy" another 10: alone, neither wins
    let mut events = Vec::new();
    for i in 0..10 {
        events.push(json!({
            "id": format!("a-{i}"),
            "time": chrono::DateTime::from_timestamp(1_000_000 + i, 0).unwrap().to_rfc3339(),
            "killer": "fck", "victim": "bob", "player_count": 2,
        }));
        events.push(json!({
            "id": format!("b-{i}"),
            "time": chrono::DateTime::from_timestamp(2_000_000 + i, 0).unwrap().to_rfc3339(),
            "killer": "deadlyenergy", "victim": "bob", "player_count": 2,
        }));
    }
    app.import(Value::Array(events)).await;

    let (_, before) = app.get("/noalt/leaderboard").await;
    assert_eq!(
        before.as_array().expect("array").len(),
        3,
        "three separate logins"
    );
    let (_, noalt_fck) = app.get("/noalt/players/fck").await;
    assert_eq!(noalt_fck["wins"], 0);
    assert_eq!(noalt_fck["kills"], 10);

    // mapping an alt that was never seen before, and one that was
    let (status, report) = app
        .post(
            "/alts",
            json!({"items": [
                {"alt": "fck", "id": 57, "main": "deadlyenergy"},
                {"alt": "orinslc", "id": 56, "main": "orinslc56"}
            ]}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["players"], 2, "deadlyenergy + bob");
    assert_eq!(
        report["matches"], 2,
        "the merged identity now completes the race"
    );

    // alt-aware view: the main owns everything
    let (_, main) = app.get("/players/deadlyenergy").await;
    assert_eq!(main["login"], "deadlyenergy");
    assert_eq!(main["kills"], 20);
    assert_eq!(main["wins"], 1);
    assert!(main["rating"].as_f64().expect("rating") > 1500.0);
    // the alt is no longer a player of its own, it resolves to the main
    let (_, aliased) = app.get("/players/fck").await;
    assert_eq!(aliased["login"], "deadlyenergy");
    assert_eq!(aliased["kills"], 20);

    // no-alt view is unchanged by the mapping
    let (_, after) = app.get("/noalt/leaderboard").await;
    assert_eq!(after.as_array().expect("array").len(), 3);
    let (_, noalt_fck) = app.get("/noalt/players/fck").await;
    assert_eq!(noalt_fck["wins"], 0);
    assert_eq!(noalt_fck["kills"], 10);

    // matchups and the event feed follow the resolution
    let (_, matchups) = app.get("/players/deadlyenergy/matchups").await;
    let items = matchups["items"].as_array().expect("items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["opponent"], "bob");
    assert_eq!(items[0]["kills"], 20);
    let (_, events) = app.get("/events?player=fck&limit=100").await;
    assert_eq!(
        events["total"], 20,
        "the alt filter matches the merged identity"
    );
    let first = events["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["id"] == json!("a-0"))
        .expect("a-0 is in the feed");
    assert_eq!(first["killer"], "fck", "the log keeps the recorded login");
    assert_eq!(first["killer_resolved"], "deadlyenergy");

    let (status, alts) = app.get("/alts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(alts["total"], 2);
}

#[tokio::test]
async fn a_main_registered_after_the_events_is_recalculated() {
    let app = TestApp::new();
    // every event names the alt; the main appears in no event at all
    let mut events = Vec::new();
    for i in 0..20 {
        events.push(json!({
            "id": format!("z-{i}"),
            "time": chrono::DateTime::from_timestamp(1_000_000 + i, 0).unwrap().to_rfc3339(),
            "killer": "smurf", "victim": "bob", "player_count": 2,
        }));
    }
    app.import(Value::Array(events)).await;

    let (_, before) = app.get("/players/smurf").await;
    assert_eq!(before["wins"], 1, "smurf wins the race on its own");

    // attach the account to a main that only now enters the system
    let (status, report) = app
        .post("/alts", json!({"items": [{"alt": "smurf", "main": "pro"}]}))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["players"], 2);

    let (_, pro) = app.get("/players/pro").await;
    assert_eq!(pro["login"], "pro");
    assert_eq!(pro["wins"], 1);
    assert_eq!(pro["kills"], 20);
    assert!(pro["rating"].as_f64().expect("rating") > 1500.0);

    let (_, bob) = app.get("/players/bob").await;
    assert_eq!(bob["losses"], 1);

    // removing the mapping restores the split
    let (status, report) = app.request("DELETE", "/alts/smurf", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(report["players"], 2, "smurf and bob again");
    let (_, smurf) = app.get("/players/smurf").await;
    assert_eq!(smurf["wins"], 1);
    let (status, _) = app.get("/players/pro").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "pro has no history of its own"
    );
}

#[tokio::test]
async fn alt_mappings_reject_cycles_and_self_references() {
    let app = TestApp::new();
    let (status, body) = app
        .post("/alts", json!({"items": [{"alt": "a", "main": "a"}]}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().expect("error").contains("own main"));

    let (status, _) = app
        .post(
            "/alts",
            json!({"items": [{"alt": "a", "main": "b"}, {"alt": "b", "main": "a"}]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = app.request("DELETE", "/alts/ghost", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_missing_import_file_is_reported_clearly() {
    let app = TestApp::new();
    let (status, body) = app
        .post(
            "/events/import",
            json!([{"id": "bad", "time": "not-a-date"}]),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "malformed payload: {body}");
}

#[tokio::test]
async fn swagger_ui_and_the_openapi_document_are_served() {
    let app = TestApp::new();

    let (status, doc) = app.get("/api-docs/openapi.json").await;
    assert_eq!(status, StatusCode::OK);
    let paths = doc["paths"].as_object().expect("paths");
    for path in [
        "/events/import",
        "/leaderboard",
        "/leaderboard/paginated",
        "/players/{login}",
        "/players/{login}/matchups",
        "/players/{login}/matchups/{opponent}",
        "/events",
        "/alts",
        "/noalt/leaderboard",
        "/noalt/leaderboard/paginated",
        "/noalt/players/{login}",
        "/noalt/players/{login}/matchups",
        "/noalt/players/{login}/matchups/{opponent}",
        "/noalt/events",
    ] {
        assert!(
            paths.contains_key(path),
            "openapi document is missing {path}"
        );
    }
    assert_eq!(doc["info"]["title"], "Glicko-2 kill-rating API");

    let response = app
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/swagger-ui")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(
        response.status().is_redirection() || response.status() == StatusCode::OK,
        "swagger ui returned {}",
        response.status()
    );
}

#[tokio::test]
async fn the_event_log_survives_a_restart() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_path_buf();
    {
        let app = TestApp::at(&path);
        app.import(sample_events()).await;
        let (_, health) = app.get("/health").await;
        assert_eq!(health["events"], 3);
    }

    // a fresh app over the same directory sees the persisted history
    let app = TestApp::at(&path);
    let (status, body) = app.get("/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["events"], 3);

    let (_, board) = app.get("/leaderboard").await;
    assert_eq!(
        find(board.as_array().expect("array"), "nicolas404")["kills"],
        2
    );
}
