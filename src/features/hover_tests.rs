use super::*;
use crate::indexer::Indexer;

fn hover_text(hover: &Hover) -> String {
    match &hover.contents {
        HoverContents::Markup(markup) => markup.value.clone(),
        _ => String::new(),
    }
}

#[test]
fn hover_resolves_constructor_and_bare_factory_receivers() {
    let indexer = Indexer::new();
    let uri = Url::parse("file:///example/Factory.kt").unwrap();
    for receiver in ["Foo()", "create()"] {
        let source = format!(
            "package example\nclass Foo {{\n    /** Instance documentation. */\n    fun action(): String = \"value\"\n}}\nfun create(): Foo = Foo()\nfun example() = {receiver}.action()\n"
        );
        indexer.index_content(&uri, &source);
        indexer.store_live_tree(&uri, &source);
        let line = source.lines().last().unwrap();
        let position = Position::new(6, line.find(".action").unwrap() as u32 + 1);
        let context = CursorContext::build(&indexer, &uri, position).unwrap();
        let hover = compute_hover(&indexer, &context, &uri, position)
            .unwrap_or_else(|| panic!("missing hover for {receiver}"));
        assert!(hover_text(&hover).contains("Instance documentation."));
    }
}

#[test]
fn hover_resolves_return_types_in_the_declaring_class_scope() {
    let indexer = Indexer::new();
    let java_uri = Url::parse("file:///example/Factories.java").unwrap();
    indexer.index_content(
        &java_uri,
        r#"package example;
class A {
    static class Result {
        /** The correct result. */
        public String action() { return null; }
    }
    public static Result make() { return null; }
}
class B {
    static class Result {
        /** The wrong result. */
        public String action() { return null; }
    }
}
"#,
    );
    let uri = Url::parse("file:///example/Example.kt").unwrap();
    let source = "package example\nfun example() = A.make().action()\n";
    indexer.index_content(&uri, source);
    indexer.store_live_tree(&uri, source);
    let position = Position::new(
        1,
        source.lines().nth(1).unwrap().find(".action").unwrap() as u32 + 1,
    );
    let context = CursorContext::build(&indexer, &uri, position).unwrap();
    let hover = compute_hover(&indexer, &context, &uri, position).expect("nested result hover");
    let text = hover_text(&hover);
    assert!(text.contains("The correct result."), "{text}");
    assert!(!text.contains("The wrong result."), "{text}");
}

#[tokio::test]
async fn hover_resolves_multiline_java_library_builder_chain() {
    use std::sync::Arc;

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
            /** Configures the JDBC template. */
            public Builder withJdbcTemplate(JdbcTemplate jdbcTemplate) { return this; }
            /** Creates the JDBC configuration. */
            public Provider.Configuration build() { return null; }
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
        /** Uses the database clock. */
        public T usingDbTime() { return null; }
    }
}
"#,
    )
    .unwrap();
    let indexer = Arc::new(Indexer::new());
    *indexer.source_paths_raw.write().unwrap() = vec![library.path().to_str().unwrap().to_owned()];
    Arc::clone(&indexer)
        .index_source_paths(workspace.path().to_owned())
        .await;
    // Source-attached dependencies also remain in the compiled JAR index.
    // Keep both copies, alongside unrelated classes with the same leaf name.
    for filename in ["Provider.java", "SqlConfiguration.java"] {
        let source_uri = Url::from_file_path(library.path().join(filename)).unwrap();
        let data = crate::resolver::ensure_file_data(&indexer, &source_uri).unwrap();
        let compiled_uri =
            Url::parse(&format!("jar:file:///dependency.jar!/app/{filename}")).unwrap();
        indexer
            .jar_files
            .insert(compiled_uri.to_string(), Arc::clone(&data));
        for symbol in &data.symbols {
            indexer
                .jar_definitions
                .entry(symbol.name.clone())
                .or_default()
                .push(Location {
                    uri: compiled_uri.clone(),
                    range: symbol.selection_range,
                });
        }
    }
    let competing_uri = Url::from_file_path(workspace.path().join("Other.java")).unwrap();
    indexer.index_content(&competing_uri, "package unrelated; public class Other { public static class Builder { public Builder withJdbcTemplate(Object wrong) { return this; } public String usingDbTime() { return null; } public String build() { return null; } } }");
    let decoy_uri = Url::from_file_path(workspace.path().join("MemoryFileStore.kt")).unwrap();
    indexer.index_content(
        &decoy_uri,
        "package unrelated\nclass URLBuilder\nval builder: URLBuilder = URLBuilder()\nfun build(): String = \"wrong\"\n",
    );
    let uri = Url::from_file_path(workspace.path().join("Example.kt")).unwrap();
    for separator in ["", "\n        "] {
        let source = format!(
            "package example\nimport app.Provider\nimport org.springframework.jdbc.core.JdbcTemplate\nfun example() =\n    Provider.Configuration{separator}.builder(){separator}.withJdbcTemplate(JdbcTemplate()){separator}.usingDbTime(){separator}.build()\n"
        );
        indexer.index_content(&uri, &source);
        indexer.store_live_tree(&uri, &source);
        for (method, expected) in [
            ("builder", "Configuration.Builder"),
            ("withJdbcTemplate", "Builder"),
            ("usingDbTime", "usingDbTime"),
            ("build", "Configuration"),
        ] {
            let needle = format!(".{method}(");
            let byte_offset = source.find(&needle).unwrap() + 1;
            let prefix = &source[..byte_offset];
            let position = Position::new(
                prefix.bytes().filter(|byte| *byte == b'\n').count() as u32,
                prefix.rsplit('\n').next().unwrap().len() as u32,
            );
            let context = CursorContext::build(&indexer, &uri, position).unwrap();
            let hover =
                compute_hover(indexer.as_ref(), &context, &uri, position).unwrap_or_else(|| {
                    panic!("missing hover for {method} with separator {separator:?}")
                });
            let text = hover_text(&hover);
            assert!(text.contains(&format!("{method}(")), "{method}: {text}");
            assert!(text.contains(expected), "{method}: {text}");
            let documentation = match method {
                "withJdbcTemplate" => Some("Configures the JDBC template."),
                "usingDbTime" => Some("Uses the database clock."),
                "build" => Some("Creates the JDBC configuration."),
                _ => None,
            };
            if let Some(documentation) = documentation {
                assert!(text.contains(documentation), "{method}: {text}");
            }
            assert!(
                !text.contains("URLBuilder") && !text.contains("wrong"),
                "{method}: {text}"
            );
        }
    }
}

#[test]
fn hover_preserves_source_jar_builder_identity_after_generic_calls() {
    let indexer = Indexer::new();
    crate::indexer::jar::index_jar_entries(
        &indexer,
        vec![
            (
                Url::parse("jar:file:///provider-sources.jar!/Provider.java").unwrap(),
                r#"package app;
import app.Base;
public class Provider {
    public static class Configuration {
        public static Configuration.Builder builder() { return null; }
        public static class Builder extends Base<Builder> {
            public Configuration build() { return null; }
        }
    }
}
"#
                .to_owned(),
            ),
            (
                Url::parse("jar:file:///base-sources.jar!/Base.java").unwrap(),
                r#"package app;
public class Base<T> {
    public T configure() { return null; }
}
"#
                .to_owned(),
            ),
            (
                Url::parse("jar:file:///other-sources.jar!/Other.java").unwrap(),
                r#"package unrelated;
public class Other {
    public static class Builder {
        public String build() { return null; }
    }
}
"#
                .to_owned(),
            ),
        ],
    );
    let uri = Url::parse("file:///example/Example.kt").unwrap();
    let source = "package example\nimport app.Provider\nfun example() = Provider.Configuration.builder().configure().build()\n";
    indexer.index_content(&uri, source);
    indexer.store_live_tree(&uri, source);
    let position = Position::new(
        2,
        source.lines().nth(2).unwrap().find(".build()").unwrap() as u32 + 1,
    );
    let context = CursorContext::build(&indexer, &uri, position).unwrap();
    let hover =
        compute_hover(&indexer, &context, &uri, position).expect("source JAR builder hover");
    let text = hover_text(&hover);
    assert!(text.contains("Configuration build()"), "{text}");
    assert!(!text.contains("String build()"), "{text}");
}

#[test]
fn hover_preserves_value_receiver_identity_in_nested_accesses() {
    let indexer = Indexer::new();
    let uri = Url::parse("file:///example/Values.kt").unwrap();
    for receiver in ["holder.foo", "holder.getFoo()"] {
        let source = format!(
            r#"package example
class Foo {{
    /** The instance action. */
    fun action(): String = "instance"
    companion object {{
        /** The companion action, never the value target. */
        fun action(): String = "companion"
        /** Only available on the companion. */
        fun missing(): String = "wrong"
    }}
}}
class Holder(val foo: Foo) {{
    fun getFoo(): Foo = foo
}}
fun example(holder: Holder) {{
    {receiver}.action()
    {receiver}.missing()
}}
"#
        );
        indexer.index_content(&uri, &source);
        indexer.store_live_tree(&uri, &source);
        for method in ["action", "missing"] {
            let line = source
                .lines()
                .position(|line| line.contains(&format!("{receiver}.{method}()")))
                .unwrap();
            let column = source
                .lines()
                .nth(line)
                .unwrap()
                .find(&format!(".{method}"))
                .unwrap()
                + 1;
            let position = Position::new(line as u32, column as u32);
            let context = CursorContext::build(&indexer, &uri, position).unwrap();
            let hover = compute_hover(&indexer, &context, &uri, position);
            if method == "action" {
                let text = hover_text(&hover.expect("instance action hover"));
                assert!(text.contains("The instance action."), "{receiver}: {text}");
                assert!(!text.contains("companion"), "{receiver}: {text}");
            } else {
                assert!(
                    hover.is_none(),
                    "must not show companion-only docs for {receiver}"
                );
            }
        }
    }
}

/// Same reported bug as goto-definition's regression test, reached through
/// hover's separate `resolve_symbol_info` path instead of
/// `find_definition`: hovering the inner `collect(block)` must not show
/// the enclosing 2-required-arg declaration's own signature.
#[test]
fn hover_does_not_show_wrong_arity_self_reference() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "class CoroutineScope\n\
               fun <T : Any> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
                   collect(block)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(2).unwrap().find("collect").unwrap() as u32;
    let position = Position::new(2, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position);
    if let Some(hover) = hover {
        let text = hover_text(&hover);
        assert!(
            !text.contains("scope: CoroutineScope"),
            "hover must not show the enclosing self declaration's own \
             signature, got: {text:?}"
        );
    }
}

/// The follow-up reported bug, once the self-shadow above was suppressed:
/// hover showed nothing at all, because nothing ever tried "this bare call
/// is really `this.collect(...)` against the enclosing extension function's
/// own receiver" — the real target is `Flow`'s own JAR-indexed interface
/// member (reached only via implicit-receiver resolution, mirroring the
/// matching `goto_definition_resolves_implicit_receiver_call_to_jar_member`
/// end-to-end test).
#[test]
fn hover_resolves_implicit_receiver_call_to_jar_member() {
    use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;

    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "package com.example\n\
               import kotlinx.coroutines.flow.Flow\n\
               class CoroutineScope\n\
               fun <T : Any> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
                   collect(block)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let jar_uri_str = "jar:file:///fake-coroutines.jar!/Flow.kt".to_string();
    let type_range = tower_lsp::lsp_types::Range {
        start: Position::new(0, 0),
        end: Position::new(0, 4),
    };
    let member_range = tower_lsp::lsp_types::Range {
        start: Position::new(1, 0),
        end: Position::new(1, 7),
    };
    let member = SymbolEntry {
        name: "collect".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::METHOD,
        visibility: Visibility::Public,
        range: member_range,
        selection_range: member_range,
        detail: "suspend fun collect(collector: FlowCollector<T>)".to_owned(),
        container: Some("Flow".to_owned()),
        params: "collector: FlowCollector<T>".to_owned(),
        param_counts: (1, 1),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    let flow_type = SymbolEntry {
        name: "Flow".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::INTERFACE,
        visibility: Visibility::Public,
        range: type_range,
        selection_range: type_range,
        detail: "interface Flow<T>".to_owned(),
        container: None,
        params: String::new(),
        param_counts: (0, 0),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    idx.jar_files.insert(
        jar_uri_str.clone(),
        Arc::new(FileData {
            symbols: vec![flow_type, member],
            source_set: SourceSet::Library,
            package: Some("kotlinx.coroutines.flow".to_owned()),
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );
    idx.jar_definitions
        .entry("Flow".to_owned())
        .or_default()
        .push(tower_lsp::lsp_types::Location {
            uri: Url::parse(&jar_uri_str).unwrap(),
            range: type_range,
        });

    let col = src.lines().nth(4).unwrap().find("collect").unwrap() as u32;
    let position = Position::new(4, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("FlowCollector"),
        "hover must show Flow's own collect member signature, got: {text:?}"
    );
    assert!(
        !text.contains("scope: CoroutineScope"),
        "hover must not show the enclosing self declaration's own \
         signature, got: {text:?}"
    );
}

/// A second, distinct manifestation of the same self-shadow bug, reported
/// after the implicit-receiver fix above shipped: an *explicit*-receiver call
/// (`triggers.collect { trigger -> ... }`, trailing-lambda-only) reaches
/// `contextual_receiver_hover` via `ctx.contextual` — populated for *any*
/// qualified reference via smart-cast narrowing, not just `it`/`this`/named
/// lambda params — which had no arity awareness at all (mirrors
/// `goto_definition_resolves_explicit_receiver_call_to_jar_member_not_self`).
#[test]
fn hover_resolves_explicit_receiver_call_to_jar_member_not_self() {
    use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};
    use std::sync::Arc;

    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Flow.kt").unwrap();
    let src = "package com.example\n\
               import kotlinx.coroutines.flow.Flow\n\
               class CoroutineScope\n\
               fun <T : Any> Flow<T>.collect(scope: CoroutineScope, block: (T) -> Unit) {\n\
                   collect(block)\n\
               }\n\
               fun useTriggers(triggers: Flow<String>) {\n\
                   triggers.collect { trigger -> println(trigger) }\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);

    let jar_uri_str = "jar:file:///fake-coroutines.jar!/Flow.kt".to_string();
    let type_range = tower_lsp::lsp_types::Range {
        start: Position::new(0, 0),
        end: Position::new(0, 4),
    };
    let member_range = tower_lsp::lsp_types::Range {
        start: Position::new(1, 0),
        end: Position::new(1, 7),
    };
    let member = SymbolEntry {
        name: "collect".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::METHOD,
        visibility: Visibility::Public,
        range: member_range,
        selection_range: member_range,
        detail: "suspend fun collect(collector: FlowCollector<T>)".to_owned(),
        container: Some("Flow".to_owned()),
        params: "collector: FlowCollector<T>".to_owned(),
        param_counts: (1, 1),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    let flow_type = SymbolEntry {
        name: "Flow".to_owned(),
        kind: tower_lsp::lsp_types::SymbolKind::INTERFACE,
        visibility: Visibility::Public,
        range: type_range,
        selection_range: type_range,
        detail: "interface Flow<T>".to_owned(),
        container: None,
        params: String::new(),
        param_counts: (0, 0),
        cold: crate::types::pack_cold_fields(vec![], String::new(), String::new(), String::new()),
        trailing_lambda: false,
        deprecated: false,
    };
    idx.jar_files.insert(
        jar_uri_str.clone(),
        Arc::new(FileData {
            symbols: vec![flow_type, member],
            source_set: SourceSet::Library,
            package: Some("kotlinx.coroutines.flow".to_owned()),
            lines: Arc::new(vec![]),
            ..Default::default()
        }),
    );
    idx.jar_definitions
        .entry("Flow".to_owned())
        .or_default()
        .push(tower_lsp::lsp_types::Location {
            uri: Url::parse(&jar_uri_str).unwrap(),
            range: type_range,
        });

    let col = src.lines().nth(7).unwrap().find("collect").unwrap() as u32;
    let position = Position::new(7, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("FlowCollector"),
        "hover must show Flow's own collect member signature, got: {text:?}"
    );
    assert!(
        !text.contains("scope: CoroutineScope"),
        "hover must not show the collect(scope, block) self-declaration's \
         own signature, got: {text:?}"
    );
}

/// Genuine same-arity self-recursion must still hover to itself — the
/// arity filter must not become a blanket "never show a same-file match."
#[test]
fn hover_shows_same_arity_self_recursion() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Factorial.kt").unwrap();
    let src = "fun factorial(n: Int): Int {\n\
                   return factorial(n - 1)\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(1).unwrap().find("factorial").unwrap() as u32;
    let position = Position::new(1, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("factorial"),
        "same-arity self-recursion must still hover to itself, got: {text:?}"
    );
}

/// Regression: hovering a smart-cast-narrowed field access (`event.event`
/// inside `is Event.OverdraftInput -> when (event.event) { ... }`) must show
/// the *specific* nested variant's own field, not a same-named sibling's.
///
/// `CursorContext::build` correctly narrows the root to `Event.OverdraftInput`
/// (confirmed via smart-cast inference), but the member lookup
/// (`resolve_qualified`'s uppercase-qualifier branch, in `resolver/resolve.rs`)
/// only anchored the search on the *outer* type's (`Event`'s) own declaration
/// line, then took whichever same-named field was textually closest —
/// `RegularInput`'s `event` field, declared before `OverdraftInput`'s in the
/// same sealed interface, not the one actually being hovered. `RegularInput`
/// is the decoy that makes this test fail without the fix; a single-variant
/// fixture would pass by accident regardless.
#[test]
fn hover_on_smart_cast_narrowed_field_shows_the_specific_variant_not_a_sibling() {
    let idx = Indexer::new();
    let uri = Url::parse("file:///t/Reducer.kt").unwrap();
    let src = "\
sealed interface Event {
    data class RegularInput(val event: RegularEvent) : Event
    data class OverdraftInput(val event: OverdraftEvent) : Event
}
sealed interface RegularEvent {
    object RegularOnClick : RegularEvent
}
sealed interface OverdraftEvent {
    object OverdraftOnClick : OverdraftEvent
}
class Reducer {
    fun reduce(event: Event) {
        when (event) {
            is Event.OverdraftInput -> when (event.event) {
                is OverdraftEvent.OverdraftOnClick -> println(\"1\")
            }
            else -> {}
        }
    }
}
";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let target_line = src
        .lines()
        .position(|line| line.contains("when (event.event)"))
        .unwrap();
    // The second `event` — the field access, not the smart-cast root.
    let col = src
        .lines()
        .nth(target_line)
        .unwrap()
        .rfind("event")
        .unwrap() as u32;
    let position = Position::new(target_line as u32, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("OverdraftInput") && !text.contains("RegularInput"),
        "must show OverdraftInput's own `event` field, not RegularEvent's \
         sibling field found by proximity to Event's own declaration, got: {text:?}"
    );
}

/// Real corpus case that motivated PR #304 (Moneta's `FormatUtil.java`),
/// reached through hover's own resolution pipeline instead of goto-def's.
/// `resolve_qualified` now hands every same-named overload to its caller
/// (`find_all_names_scoped_to_container`, see PR #304), in `file_data.symbols`
/// order — reverse source order for Java, so the 3-arg (last-declared)
/// overload sorts first. Before this fix, `locate_symbol`
/// (`src/indexer/resolution.rs`) picked `.into_iter().next()` with no
/// shape-awareness at all, so hovering the CALL to the 2-arg overload always
/// showed the 3-arg overload's own signature instead.
#[test]
fn hover_on_qualified_call_shows_the_called_overload_not_an_arbitrary_one() {
    let idx = Indexer::new();
    let java_uri = Url::parse("file:///app/FormatUtil.java").unwrap();
    idx.index_content(
        &java_uri,
        concat!(
            "package app;\n",
            "public class FormatUtil {\n",
            "    public static String formatAmount(java.math.BigDecimal a) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b, boolean c) { return null; }\n",
            "}\n",
        ),
    );

    let uri = Url::parse("file:///app/Caller.kt").unwrap();
    let src = "package app\n\
               class Caller {\n\
                   fun run(amount: java.math.BigDecimal) {\n\
                       FormatUtil.formatAmount(amount, 2)\n\
                   }\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(3).unwrap().find("formatAmount").unwrap() as u32;
    let position = Position::new(3, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position).expect("expected a hover result");
    let text = hover_text(&hover);
    assert!(
        text.contains("int b") && !text.contains("boolean c"),
        "hovering the 2-arg call must show the 2-arg overload's own \
         signature, not the 3-arg (or 1-arg) overload's, got: {text:?}"
    );
}

/// The sibling gap left open by PR #304: a qualified reference with NO
/// derivable call shape (here, a bare property-style reference — no call
/// parens at all) that still resolves to more than one overload candidate
/// must decline rather than silently showing an arbitrary overload's docs.
#[test]
fn hover_on_ambiguous_qualified_reference_without_call_declines_rather_than_guessing() {
    let idx = Indexer::new();
    let java_uri = Url::parse("file:///app/FormatUtil.java").unwrap();
    idx.index_content(
        &java_uri,
        concat!(
            "package app;\n",
            "public class FormatUtil {\n",
            "    public static String formatAmount(java.math.BigDecimal a) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b) { return null; }\n",
            "    public static String formatAmount(java.math.BigDecimal a, int b, boolean c) { return null; }\n",
            "}\n",
        ),
    );

    let uri = Url::parse("file:///app/Caller.kt").unwrap();
    let src = "package app\n\
               class Caller {\n\
                   val ref = FormatUtil.formatAmount\n\
               }\n";
    idx.index_content(&uri, src);
    idx.store_live_tree(&uri, src);
    let col = src.lines().nth(2).unwrap().find("formatAmount").unwrap() as u32;
    let position = Position::new(2, col);
    let ctx = CursorContext::build(&idx, &uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &uri, position);
    assert!(
        hover.is_none(),
        "an ambiguous qualified reference with no derivable call shape must \
         decline rather than showing an arbitrary overload's docs, got: {:?}",
        hover.map(|h| hover_text(&h))
    );
}

/// The reversal of the hover regression that PR #304's own extension-registry
/// fix introduced (see `ambiguous_member_extension_name_collision_returns_
/// the_whole_candidate_set` in `src/resolver/tests.rs`, Task 4 of the
/// 2026-09-14 extension-registry-overload-collapse plan): two unrelated
/// interfaces each declare a same-arity member extension named `weight` on
/// `Modifier`. Resolution now correctly surfaces both as a candidate set
/// instead of collapsing to one arbitrary match, but since both candidates
/// share the same name and the same declared arity, hover must still render
/// (using the first) rather than declining outright.
#[test]
fn hover_renders_for_a_same_arity_extension_collision() {
    let idx = Indexer::new();
    let first_uri = Url::parse("file:///compose/ColumnScope.kt").unwrap();
    idx.index_content(
        &first_uri,
        concat!(
            "package androidx.compose.foundation.layout\n",
            "interface ColumnScope {\n",
            "    fun Modifier.weight(weight: Float): Modifier\n",
            "}\n",
        ),
    );
    let second_uri = Url::parse("file:///lib/UnrelatedScope.kt").unwrap();
    idx.index_content(
        &second_uri,
        concat!(
            "package com.example.lib\n",
            "interface UnrelatedScope {\n",
            "    fun Modifier.weight(weight: Float): Modifier\n",
            "}\n",
        ),
    );

    let use_uri = Url::parse("file:///app/Screen.kt").unwrap();
    let src = "package app\n\
               fun screen() {\n\
                   Modifier.weight(1f)\n\
               }\n";
    idx.index_content(&use_uri, src);
    idx.store_live_tree(&use_uri, src);

    let col = src.lines().nth(2).unwrap().find("weight").unwrap() as u32;
    let position = Position::new(2, col);
    let ctx = CursorContext::build(&idx, &use_uri, position).unwrap();

    let hover = compute_hover(&idx, &ctx, &use_uri, position);
    assert!(
        hover.is_some(),
        "a same-arity extension-function collision (the Modifier.weight \
         shape) must still render hover using the first candidate, not \
         decline outright"
    );
}
