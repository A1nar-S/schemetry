use super::*;

fn rule(id: i64, action: FilterAction, match_type: MatchType, pattern: &str, enabled: bool) -> TableFilterRule {
    TableFilterRule { id, action, match_type, pattern: pattern.to_string(), enabled }
}

// ── like_pattern ─────────────────────────────────────────────────

#[test]
fn like_pattern_prefix_appends_wildcard() {
    assert_eq!(like_pattern(MatchType::Prefix, "arch_"), "ARCH_%");
}

#[test]
fn like_pattern_suffix_prepends_wildcard() {
    assert_eq!(like_pattern(MatchType::Suffix, "_arch"), "%_ARCH");
}

#[test]
fn like_pattern_contains_wraps_wildcards() {
    assert_eq!(like_pattern(MatchType::Contains, "_test_"), "%_TEST_%");
}

#[test]
fn like_pattern_exact_has_no_wildcard() {
    assert_eq!(like_pattern(MatchType::Exact, "audit_log"), "AUDIT_LOG");
}

// ── build_predicate ──────────────────────────────────────────────

#[test]
fn build_predicate_empty_rules_yields_empty_fragment() {
    let (sql, binds) = build_predicate(&[], "TABLE_NAME", 2, ParamStyle::Colon);
    assert_eq!(sql, "");
    assert!(binds.is_empty());
}

#[test]
fn build_predicate_disabled_rule_is_ignored() {
    let rules = vec![rule(1, FilterAction::Exclude, MatchType::Prefix, "ARCH_", false)];
    let (sql, binds) = build_predicate(&rules, "TABLE_NAME", 2, ParamStyle::Colon);
    assert_eq!(sql, "");
    assert!(binds.is_empty());
}

#[test]
fn build_predicate_single_exclude_rule() {
    let rules = vec![rule(1, FilterAction::Exclude, MatchType::Prefix, "ARCH_", true)];
    let (sql, binds) = build_predicate(&rules, "TABLE_NAME", 2, ParamStyle::Colon);
    assert_eq!(sql, " AND UPPER(TABLE_NAME) NOT LIKE :2");
    assert_eq!(binds, vec!["ARCH_%".to_string()]);
}

#[test]
fn build_predicate_multiple_exclude_rules_and_together_with_sequential_binds() {
    let rules = vec![
        rule(1, FilterAction::Exclude, MatchType::Prefix, "ARCH_", true),
        rule(2, FilterAction::Exclude, MatchType::Suffix, "_ARCH", true),
    ];
    let (sql, binds) = build_predicate(&rules, "TABLE_NAME", 3, ParamStyle::Colon);
    assert_eq!(
        sql,
        " AND UPPER(TABLE_NAME) NOT LIKE :3 AND UPPER(TABLE_NAME) NOT LIKE :4"
    );
    assert_eq!(binds, vec!["ARCH_%".to_string(), "%_ARCH".to_string()]);
}

#[test]
fn build_predicate_include_rules_are_ored_together() {
    let rules = vec![
        rule(1, FilterAction::Include, MatchType::Prefix, "APP_", true),
        rule(2, FilterAction::Include, MatchType::Prefix, "CORE_", true),
    ];
    let (sql, binds) = build_predicate(&rules, "TABLE_NAME", 2, ParamStyle::Colon);
    assert_eq!(
        sql,
        " AND (UPPER(TABLE_NAME) LIKE :2 OR UPPER(TABLE_NAME) LIKE :3)"
    );
    assert_eq!(binds, vec!["APP_%".to_string(), "CORE_%".to_string()]);
}

#[test]
fn build_predicate_combines_exclude_and_include() {
    let rules = vec![
        rule(1, FilterAction::Exclude, MatchType::Prefix, "ARCH_", true),
        rule(2, FilterAction::Include, MatchType::Prefix, "APP_", true),
    ];
    let (sql, binds) = build_predicate(&rules, "TABLE_NAME", 2, ParamStyle::Colon);
    assert_eq!(
        sql,
        " AND UPPER(TABLE_NAME) NOT LIKE :2 AND (UPPER(TABLE_NAME) LIKE :3)"
    );
    assert_eq!(binds, vec!["ARCH_%".to_string(), "APP_%".to_string()]);
}

// ── action / match_type string round-trip ───────────────────────

#[test]
fn action_str_round_trips() {
    assert_eq!(SqliteFilterRuleRepository::parse_action(SqliteFilterRuleRepository::action_str(FilterAction::Exclude)), FilterAction::Exclude);
    assert_eq!(SqliteFilterRuleRepository::parse_action(SqliteFilterRuleRepository::action_str(FilterAction::Include)), FilterAction::Include);
}

#[test]
fn match_type_str_round_trips() {
    for mt in [MatchType::Prefix, MatchType::Suffix, MatchType::Contains, MatchType::Exact] {
        assert_eq!(SqliteFilterRuleRepository::parse_match_type(SqliteFilterRuleRepository::match_type_str(mt)), mt);
    }
}
