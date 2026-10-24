//! A local SQLite log of commands ExecGuard has seen run.
//!
//! The shell hooks call into this (through `execguard record`) *after* a
//! command finishes, since the exit code and how long it took are only
//! known at that point. Before anything is written, the command text is
//! [redacted](redact) and normalized, so secrets never reach the database
//! and small formatting differences (`rm  -rf  x` vs `rm -rf x`) collapse
//! into the same `commands` row.
//!
//! This crate never decides policy — it only records what already
//! happened. If a write fails, callers should treat that as a shrug, not a
//! reason to fail a command that already ran: see
//! `dev-doc/HANDOFF.md` §4.3's fail-open philosophy.

mod redact;

use rusqlite::{params, Connection};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS commands (
    id         INTEGER PRIMARY KEY,
    text       TEXT NOT NULL UNIQUE,
    times_seen INTEGER NOT NULL DEFAULT 1,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS executions (
    id           INTEGER PRIMARY KEY,
    command_id   INTEGER NOT NULL REFERENCES commands(id),
    cwd          TEXT NOT NULL,
    shell        TEXT NOT NULL,
    ran_at       INTEGER NOT NULL,
    exit_code    INTEGER NOT NULL,
    duration_ms  INTEGER NOT NULL,
    outcome      TEXT NOT NULL,
    -- The Git repository containing `cwd`, if any; NULL outside one.
    project_root TEXT
);
";

/// A connection to the history database, with its tables already created.
pub struct Store {
    conn: Connection,
}

/// Everything needed to record one command that just finished running.
pub struct NewExecution<'a> {
    /// The command as the user typed it. Redacted before it is ever stored.
    pub command: &'a str,
    pub cwd: &'a str,
    pub shell: &'a str,
    pub exit_code: i32,
    pub duration_ms: i64,
    /// The policy verdict ExecGuard gave it, e.g. `"WARN"`.
    pub outcome: &'a str,
}

impl Store {
    /// Opens the database at `path`, creating the file and its tables if
    /// they don't exist yet.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Store { conn })
    }

    /// Where the history database lives by default: a per-user data
    /// directory (`%LOCALAPPDATA%\execguard` on Windows, otherwise
    /// `$XDG_DATA_HOME/execguard` or `~/.local/share/execguard`).
    pub fn default_path() -> Result<PathBuf, StorageError> {
        let data_dir = if cfg!(windows) {
            std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        } else {
            std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
        };

        data_dir.map(|dir| dir.join("execguard").join("history.db")).ok_or(StorageError::NoDataDirectory)
    }

    /// Redacts and normalizes `execution.command`, then records it: the
    /// `commands` row is created or its `times_seen`/`last_seen` updated,
    /// and a new `executions` row is added pointing at it.
    pub fn record(&self, execution: NewExecution) -> Result<(), StorageError> {
        let text = normalize(&redact::redact(execution.command));
        let command_id = self.upsert_command(&text)?;
        let project_root = find_git_root(Path::new(execution.cwd));

        self.conn.execute(
            "INSERT INTO executions (command_id, cwd, shell, ran_at, exit_code, duration_ms, outcome, project_root)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                command_id,
                execution.cwd,
                execution.shell,
                now_ms(),
                execution.exit_code,
                execution.duration_ms,
                execution.outcome,
                project_root,
            ],
        )?;

        Ok(())
    }

    /// Runs a history lookup: the most recent executions, optionally
    /// narrowed to ones whose (redacted) text matches `query.term` and/or
    /// whose project is `query.project_root`.
    pub fn query(&self, query: Query) -> Result<Vec<HistoryEntry>, StorageError> {
        let mut sql = String::from(
            "SELECT c.text, e.cwd, e.shell, e.ran_at, e.exit_code, e.duration_ms, e.outcome, e.project_root
             FROM executions e JOIN commands c ON c.id = e.command_id
             WHERE 1 = 1", // a hook so every real filter below can just start with "AND"
        );
        if query.term.is_some() {
            sql.push_str(" AND c.text LIKE :term");
        }
        if query.project_root.is_some() {
            sql.push_str(" AND e.project_root = :project_root");
        }
        sql.push_str(" ORDER BY e.ran_at DESC LIMIT :limit");

        let pattern = query.term.map(|term| format!("%{term}%"));
        let mut params: Vec<(&str, &dyn rusqlite::ToSql)> = Vec::new();
        if let Some(pattern) = &pattern {
            params.push((":term", pattern));
        }
        if let Some(project_root) = &query.project_root {
            params.push((":project_root", project_root));
        }
        params.push((":limit", &query.limit));

        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params.as_slice(), |row| {
            Ok(HistoryEntry {
                command: row.get(0)?,
                cwd: row.get(1)?,
                shell: row.get(2)?,
                ran_at_ms: row.get(3)?,
                exit_code: row.get(4)?,
                duration_ms: row.get(5)?,
                outcome: row.get(6)?,
                project_root: row.get(7)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(StorageError::from)
    }

    /// Finds the `commands` row for `text`, creating it if this is the
    /// first time it's been seen, and returns its id either way.
    fn upsert_command(&self, text: &str) -> Result<i64, StorageError> {
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO commands (text, times_seen, first_seen, last_seen) VALUES (?1, 1, ?2, ?2)
             ON CONFLICT(text) DO UPDATE SET times_seen = times_seen + 1, last_seen = ?2",
            params![text, now],
        )?;

        let id = self.conn.query_row("SELECT id FROM commands WHERE text = ?1", params![text], |row| row.get(0))?;
        Ok(id)
    }
}

/// What to look for in [`Store::query`]. Every field is optional except
/// `limit`: leaving `term`/`project_root` as `None` just means "don't
/// filter on that."
pub struct Query<'a> {
    /// Matched as a substring against the stored (redacted) command text.
    pub term: Option<&'a str>,
    /// Matched exactly against the Git repository root an execution was
    /// recorded under (see [`find_git_root`]).
    pub project_root: Option<&'a str>,
    pub limit: i64,
}

/// One row of history: a command that ran, and what happened when it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub command: String,
    pub cwd: String,
    pub shell: String,
    pub ran_at_ms: i64,
    pub exit_code: i32,
    pub duration_ms: i64,
    pub outcome: String,
    pub project_root: Option<String>,
}

/// Walks up from `dir` looking for a `.git` entry -- a directory for an
/// ordinary clone, or a file for a submodule/worktree (which points at the
/// real one elsewhere). Returns the repository's top-level directory
/// (normalized, see [`normalize_path`]), or `None` if `dir` isn't inside one.
pub fn find_git_root(dir: &Path) -> Option<String> {
    let mut current = Some(dir);
    while let Some(d) = current {
        if d.join(".git").exists() {
            return Some(normalize_path(d));
        }
        current = d.parent();
    }
    None
}

/// Normalizes a path to one comparable string. The same directory can
/// arrive spelled two different but equally valid ways -- e.g. MSYS2 bash
/// rewrites `$(pwd)` to a forward-slash Windows path (`C:/Users/...`)
/// before calling a native `.exe`, while `env::current_dir()` returns the
/// backslash form -- so without this, the same project could fail to
/// match itself. On Windows (case-insensitive, `/` and `\` both work),
/// this lowercases and switches to `/`; elsewhere, paths are already
/// case-sensitive and `/`-only, so it's left as-is.
fn normalize_path(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.to_ascii_lowercase().replace('\\', "/")
    } else {
        text
    }
}

/// Collapses runs of whitespace into single spaces and trims the ends, so
/// `rm  -rf   build` and `rm -rf build` dedupe to the same `commands` row.
fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

/// Something went wrong opening or writing to the history database.
/// This is never a reason to stop a command that already ran — see this
/// crate's top-level docs.
#[derive(Debug)]
pub enum StorageError {
    Sqlite(rusqlite::Error),
    Io(std::io::Error),
    /// `default_path` couldn't find a per-user data directory to use.
    NoDataDirectory,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageError::Sqlite(e) => write!(f, "database error: {e}"),
            StorageError::Io(e) => write!(f, "filesystem error: {e}"),
            StorageError::NoDataDirectory => write!(f, "could not find a per-user data directory"),
        }
    }
}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        StorageError::Sqlite(e)
    }
}

impl From<std::io::Error> for StorageError {
    fn from(e: std::io::Error) -> Self {
        StorageError::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_db(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("execguard-test-{name}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn execution(command: &str) -> NewExecution<'_> {
        NewExecution {
            command,
            cwd: "/home/ada/project",
            shell: "bash",
            exit_code: 0,
            duration_ms: 12,
            outcome: "ALLOW",
        }
    }

    #[test]
    fn opening_creates_the_file_and_tables() {
        let path = temp_db("open");
        Store::open(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn opening_twice_does_not_fail() {
        let path = temp_db("reopen");
        Store::open(&path).unwrap();
        Store::open(&path).unwrap();
    }

    #[test]
    fn recording_a_new_command_creates_one_row_each() {
        let path = temp_db("record-new");
        let store = Store::open(&path).unwrap();
        store.record(execution("rm -rf build")).unwrap();

        let commands: i64 = store.conn.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0)).unwrap();
        let executions: i64 = store.conn.query_row("SELECT COUNT(*) FROM executions", [], |r| r.get(0)).unwrap();
        assert_eq!(commands, 1);
        assert_eq!(executions, 1);
    }

    #[test]
    fn repeating_a_command_reuses_its_row_and_bumps_times_seen() {
        let path = temp_db("record-repeat");
        let store = Store::open(&path).unwrap();
        store.record(execution("rm -rf build")).unwrap();
        store.record(execution("rm  -rf   build")).unwrap(); // same, once normalized

        let commands: i64 = store.conn.query_row("SELECT COUNT(*) FROM commands", [], |r| r.get(0)).unwrap();
        let executions: i64 = store.conn.query_row("SELECT COUNT(*) FROM executions", [], |r| r.get(0)).unwrap();
        let times_seen: i64 = store.conn.query_row("SELECT times_seen FROM commands", [], |r| r.get(0)).unwrap();
        assert_eq!(commands, 1);
        assert_eq!(executions, 2);
        assert_eq!(times_seen, 2);
    }

    #[test]
    fn a_secret_never_reaches_the_database() {
        let path = temp_db("record-secret");
        let store = Store::open(&path).unwrap();
        store.record(execution("curl --token abc.super.secret example.com")).unwrap();

        let stored: String = store.conn.query_row("SELECT text FROM commands", [], |r| r.get(0)).unwrap();
        assert!(!stored.contains("abc.super.secret"), "secret leaked into storage: {stored}");
        assert_eq!(stored, "curl --token *** example.com");
    }

    #[test]
    fn execution_fields_are_stored() {
        let path = temp_db("record-fields");
        let store = Store::open(&path).unwrap();
        store
            .record(NewExecution {
                command: "rm -rf build",
                cwd: "/home/ada/project",
                shell: "zsh",
                exit_code: 0,
                duration_ms: 42,
                outcome: "WARN",
            })
            .unwrap();

        let (cwd, shell, exit_code, duration_ms, outcome): (String, String, i32, i64, String) = store
            .conn
            .query_row("SELECT cwd, shell, exit_code, duration_ms, outcome FROM executions", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap();

        assert_eq!(cwd, "/home/ada/project");
        assert_eq!(shell, "zsh");
        assert_eq!(exit_code, 0);
        assert_eq!(duration_ms, 42);
        assert_eq!(outcome, "WARN");
    }

    fn temp_dir(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("execguard-test-dir-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn find_git_root_detects_a_repo_from_inside_and_below_it() {
        let repo = temp_dir("repo");
        let nested = repo.join("src").join("deeper");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        assert_eq!(find_git_root(&repo), Some(normalize_path(&repo)));
        assert_eq!(find_git_root(&nested), Some(normalize_path(&repo)));

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn find_git_root_recognises_a_worktree_gitfile_too() {
        // In a submodule/worktree, `.git` is a FILE pointing elsewhere,
        // not a directory. Either one should count.
        let repo = temp_dir("worktree");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join(".git"), "gitdir: ../somewhere/else").unwrap();

        assert_eq!(find_git_root(&repo), Some(normalize_path(&repo)));

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn find_git_root_matches_regardless_of_slash_style() {
        // The exact mismatch a real MSYS2-bash vs. native-Windows path
        // would hit: two different spellings of the same directory.
        let repo = temp_dir("slash-style");
        std::fs::create_dir_all(repo.join(".git")).unwrap();

        let forward_slashed = repo.to_string_lossy().replace('\\', "/");
        assert_eq!(find_git_root(&repo), find_git_root(Path::new(&forward_slashed)));

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn find_git_root_is_none_outside_any_repo() {
        let outside = temp_dir("not-a-repo");
        std::fs::create_dir_all(&outside).unwrap();

        assert_eq!(find_git_root(&outside), None);

        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn recording_inside_a_repo_stores_its_root() {
        let repo = temp_dir("record-repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = repo.to_string_lossy().into_owned();

        let path = temp_db("record-project-root");
        let store = Store::open(&path).unwrap();
        store
            .record(NewExecution { command: "ls", cwd: &cwd, shell: "bash", exit_code: 0, duration_ms: 1, outcome: "ALLOW" })
            .unwrap();

        let stored: Option<String> =
            store.conn.query_row("SELECT project_root FROM executions", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, Some(normalize_path(&repo)));

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn recording_outside_a_repo_stores_no_project_root() {
        let path = temp_db("record-no-project-root");
        let store = Store::open(&path).unwrap();
        store.record(execution("ls")).unwrap(); // "/home/ada/project" has no real .git here

        let stored: Option<String> =
            store.conn.query_row("SELECT project_root FROM executions", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, None);
    }

    #[test]
    fn query_with_no_filters_returns_everything_newest_first() {
        let path = temp_db("query-all");
        let store = Store::open(&path).unwrap();
        store.record(execution("rm -rf build")).unwrap();
        store.record(execution("git push --force")).unwrap();

        let entries = store.query(Query { term: None, project_root: None, limit: 10 }).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].command, "git push --force"); // most recent first
        assert_eq!(entries[1].command, "rm -rf build");
    }

    #[test]
    fn query_term_filters_by_substring() {
        let path = temp_db("query-term");
        let store = Store::open(&path).unwrap();
        store.record(execution("rm -rf build")).unwrap();
        store.record(execution("git push --force")).unwrap();

        let entries = store.query(Query { term: Some("push"), project_root: None, limit: 10 }).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "git push --force");
    }

    #[test]
    fn query_limit_caps_the_results() {
        let path = temp_db("query-limit");
        let store = Store::open(&path).unwrap();
        for i in 0..5 {
            store.record(execution(&format!("echo {i}"))).unwrap();
        }

        let entries = store.query(Query { term: None, project_root: None, limit: 2 }).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn query_project_root_filters_to_that_project_only() {
        let repo = temp_dir("query-project");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let cwd = repo.to_string_lossy().into_owned();

        let path = temp_db("query-project-db");
        let store = Store::open(&path).unwrap();
        store.record(execution("rm -rf build")).unwrap(); // no project
        store
            .record(NewExecution {
                command: "git push --force",
                cwd: &cwd,
                shell: "bash",
                exit_code: 0,
                duration_ms: 1,
                outcome: "CONFIRM",
            })
            .unwrap();

        // The query filter goes through the same normalization as what
        // gets stored, the way the CLI's `--here` resolves it.
        let normalized = find_git_root(&repo).unwrap();
        let entries = store.query(Query { term: None, project_root: Some(&normalized), limit: 10 }).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].command, "git push --force");

        std::fs::remove_dir_all(&repo).unwrap();
    }
}
