use super::{cache_hit, completion_cache_key, param_names_from_sig, split_prefix, store_in_cache};
use crate::indexer::Indexer;
use tower_lsp::lsp_types::{CompletionItem, Url};

/// Regression for a TOCTOU race between a completion request and a
/// concurrent, non-JAR file reindex sharing the single-slot `last_completion`
/// cache: `run_completions` captures `completion_epoch` at the START of the
/// request (before computing its result); `index_content` (the debounced
/// live-edit reindex path) clears `last_completion` when content changes but
/// — before this fix — never bumped `completion_epoch`, only JAR-population
/// paths did (`Indexer::invalidate_completion_cache`). So a request that
/// started computing against stale (pre-edit) data, and only finishes after
/// a concurrent reindex has already cleared the cache, would pass
/// `store_in_cache`'s epoch check (unchanged) and re-populate the
/// just-cleared cache with its stale result — exactly the shape of the
/// "newly added member doesn't show up" symptom reported from live,
/// keystroke-interleaved editing (not reproducible via a single sequential
/// request well after a single finished reindex).
///
/// This test reproduces the race SHAPE directly (not just sequential
/// store-then-fetch, which can't tell "invalidated correctly" apart from
/// "invalidated then reclobbered"): capture the epoch as of before an edit,
/// perform the edit (a real reindex), then attempt to store a result computed
/// under the pre-edit epoch — the store must be rejected.
#[test]
fn store_in_cache_rejects_stale_write_racing_a_concurrent_reindex() {
    let indexer = Indexer::new();
    let uri = Url::parse("file:///test/Race.kt").unwrap();
    indexer.index_content(&uri, "class Widget {\n    fun existing() {}\n}\n");

    // Simulate the epoch a completion request captures at its own start,
    // before it begins computing (mirrors `run_completions`'s `let epoch = …`).
    let epoch_at_request_start = indexer
        .completion_epoch
        .load(std::sync::atomic::Ordering::Acquire);

    // Concurrently, the user's edit lands and the debounced reindex runs —
    // content actually changes, so `last_completion` is cleared.
    indexer.index_content(
        &uri,
        "class Widget {\n    fun existing() {}\n    fun freshlyAdded() {}\n}\n",
    );

    // The original (now-stale) request finally finishes computing against the
    // OLD symbol table and tries to persist its result under the epoch it
    // captured before the reindex.
    let stale_items = vec![CompletionItem {
        label: "existing".to_string(),
        ..Default::default()
    }];
    let key = completion_cache_key(&uri, "    w.", 1);
    store_in_cache(
        &indexer,
        key.clone(),
        &stale_items,
        false,
        epoch_at_request_start,
    );

    assert!(
        cache_hit(&indexer, &key).is_none(),
        "a completion result computed before a concurrent reindex must not \
         be allowed to re-populate the cache after that reindex cleared it"
    );
}

#[tokio::test]
async fn completes_multiline_java_library_builder_chains() {
    use std::sync::Arc;
    use tower_lsp::lsp_types::{CompletionResponse, Position};

    let library = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    std::fs::write(
        library.path().join("Provider.java"),
        r#"package app;
import app.SqlConfiguration.SqlConfigurationBuilder;
public class Provider {
    public static final class Configuration extends SqlConfiguration {
        public static Configuration.Builder builder() { return new Builder(); }
        public static final class Builder extends SqlConfigurationBuilder<Builder> {
            public Builder withTemplate(Object template) { return this; }
            public Configuration build() { return null; }
        }
    }
}
"#,
    )
    .unwrap();
    std::fs::write(
        library.path().join("SqlConfiguration.java"),
        r#"package app;
public class SqlConfiguration {
    public abstract static class SqlConfigurationBuilder<T extends SqlConfigurationBuilder<T>> {
        public T usingDbTime() { return null; }
    }
}
"#,
    )
    .unwrap();

    let indexer = Arc::new(Indexer::new());
    *indexer.source_paths_raw.write().unwrap() =
        vec![library.path().to_str().unwrap().to_owned()];
    Arc::clone(&indexer)
        .index_source_paths(workspace.path().to_owned())
        .await;
    let uri = Url::from_file_path(workspace.path().join("Example.kt")).unwrap();

    for tail in [".", ".u", ".usingDbTime().", ".usingDbTime().b"] {
        let source = format!(
            "package example\nimport app.Provider\nfun example() =\n    Provider.Configuration\n        .builder()\n        .withTemplate(Any())\n        {tail}\n"
        );
        indexer.index_content(&uri, &source);
        indexer.store_live_tree(&uri, &source);
        let position = Position::new(6, 8 + tail.len() as u32);
        let response = super::compute_completions(&uri, position, true, indexer.as_ref())
            .expect("Java builder chain should offer completions");
        let items = match response {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => list.items,
        };
        for expected in ["build", "usingDbTime", "withTemplate"] {
            assert!(
                items.iter().any(|item| item.label == expected),
                "missing {expected} after {tail}: {items:?}"
            );
        }
    }
}

#[test]
fn split_prefix_after_dot() {
    let (prefix, before_prefix) = split_prefix("foo.bar");
    assert_eq!(prefix, "bar");
    assert_eq!(before_prefix, "foo.");
}

#[test]
fn split_prefix_bare() {
    let (prefix, before_prefix) = split_prefix("someIdent");
    assert_eq!(prefix, "someIdent");
    assert_eq!(before_prefix, "");
}

// ── param_names_from_sig ──────────────────────────────────────────────────────

#[test]
fn param_names_basic() {
    assert_eq!(
        param_names_from_sig("name: String, age: Int"),
        vec!["name", "age"]
    );
}

#[test]
fn param_names_with_defaults() {
    assert_eq!(
        param_names_from_sig(
            "text: String, modifier: Modifier = Modifier, color: Color = Color.Unspecified"
        ),
        vec!["text", "modifier", "color"]
    );
}

#[test]
fn param_names_with_annotation() {
    assert_eq!(
        param_names_from_sig("@Composable content: @Composable () -> Unit"),
        vec!["content"]
    );
}

#[test]
fn param_names_vararg() {
    assert_eq!(param_names_from_sig("vararg items: String"), vec!["items"]);
}

#[test]
fn param_names_skips_this() {
    // Extension receiver `this@Foo` should not produce a named arg
    assert_eq!(param_names_from_sig("this: Foo, value: Int"), vec!["value"]);
}

#[test]
fn param_names_empty() {
    let result = param_names_from_sig("");
    assert!(result.is_empty());
}
