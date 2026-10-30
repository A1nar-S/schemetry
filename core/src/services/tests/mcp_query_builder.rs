use serde_json::json;

use super::*;

fn cols(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

fn filter(column: &str, op: FilterOp, value: Option<Value>) -> Filter {
    Filter {
        column: column.to_string(),
        op,
        value,
    }
}

fn build_for(
    db: DbType,
    columns: &[&str],
    filters: &[Filter],
    order_by: &[OrderBy],
    limit: u32,
) -> Result<BuiltQuery, String> {
    let columns = cols(columns);
    build(
        &TableQuery {
            schema: "app",
            table: "orders",
            columns: &columns,
            filters,
            order_by,
            limit,
        },
        db,
    )
}

#[test]
fn oracle_queries_upper_case_quote_and_wrap_with_rownum() {
    let built = build_for(DbType::Oracle, &["id", "status"], &[], &[], 50).unwrap();
    assert_eq!(
        built.sql,
        "SELECT * FROM (SELECT \"ID\", \"STATUS\" FROM \"APP\".\"ORDERS\") WHERE ROWNUM <= 51"
    );
    assert!(built.binds.is_empty());
}

#[test]
fn postgres_queries_keep_case_and_use_limit_plus_one() {
    let built = build_for(DbType::Postgres, &["Id"], &[], &[], 10).unwrap();
    assert_eq!(built.sql, "SELECT \"Id\" FROM \"app\".\"orders\" LIMIT 11");
}

#[test]
fn oracle_filters_become_bind_variables() {
    let filters = [
        filter("status", FilterOp::Eq, Some(json!("O'Brien"))),
        filter("total", FilterOp::Ge, Some(json!(10))),
        filter("id", FilterOp::In, Some(json!([1, 2, "x"]))),
    ];
    let built = build_for(DbType::Oracle, &["id"], &filters, &[], 5).unwrap();
    assert!(
        built
            .sql
            .contains("WHERE \"STATUS\" = :1 AND \"TOTAL\" >= :2 AND \"ID\" IN (:3, :4, :5)")
    );
    assert_eq!(built.binds, ["O'Brien", "10", "1", "2", "x"]);
    assert!(!built.sql.contains("O'Brien"));
}

#[test]
fn postgres_filters_use_escaped_literals() {
    let filters = [
        filter("name", FilterOp::Eq, Some(json!("O'Brien"))),
        filter("note", FilterOp::Like, Some(json!("%'; DROP TABLE x; --"))),
    ];
    let built = build_for(DbType::Postgres, &["id"], &filters, &[], 5).unwrap();
    assert!(built.sql.contains("\"name\" = 'O''Brien'"));
    assert!(built.sql.contains("\"note\" LIKE '%''; DROP TABLE x; --'"));
    assert!(built.binds.is_empty());
}

#[test]
fn null_checks_need_no_value() {
    let filters = [
        filter("a", FilterOp::IsNull, None),
        filter("b", FilterOp::IsNotNull, None),
    ];
    let built = build_for(DbType::Postgres, &["a"], &filters, &[], 5).unwrap();
    assert!(built.sql.contains("\"a\" IS NULL AND \"b\" IS NOT NULL"));
}

#[test]
fn order_by_supports_descending() {
    let order = [
        OrderBy {
            column: "total".into(),
            descending: true,
        },
        OrderBy {
            column: "id".into(),
            descending: false,
        },
    ];
    let built = build_for(DbType::Postgres, &["id"], &[], &order, 5).unwrap();
    assert!(
        built
            .sql
            .contains("ORDER BY \"total\" DESC, \"id\" LIMIT 6")
    );
}

#[test]
fn booleans_render_per_engine() {
    let filters = [filter("active", FilterOp::Eq, Some(json!(true)))];
    let oracle = build_for(DbType::Oracle, &["id"], &filters, &[], 1).unwrap();
    assert_eq!(oracle.binds, ["1"]);
    let pg = build_for(DbType::Postgres, &["id"], &filters, &[], 1).unwrap();
    assert!(pg.sql.contains("\"active\" = 'true'"));
}

#[test]
fn identifiers_must_be_plain() {
    for bad in [
        "a b", "a;b", "a\"b", "a.b", "1abc", "", "a-b", "a'b", "tab\tle",
    ] {
        assert!(!valid_identifier(bad), "{bad:?}");
        let columns = cols(&[bad]);
        let result = build(
            &TableQuery {
                schema: "app",
                table: "orders",
                columns: &columns,
                filters: &[],
                order_by: &[],
                limit: 1,
            },
            DbType::Postgres,
        );
        assert!(result.is_err(), "column {bad:?}");
    }
    assert!(valid_identifier("ORDER_ITEMS$1"));
    assert!(valid_identifier("_x#"));

    let columns = cols(&["id"]);
    let table_err = build(
        &TableQuery {
            schema: "app",
            table: "orders; drop table x",
            columns: &columns,
            filters: &[],
            order_by: &[],
            limit: 1,
        },
        DbType::Oracle,
    );
    assert!(table_err.is_err());
}

#[test]
fn filter_and_order_columns_are_validated_too() {
    let bad_filter = [filter("a b", FilterOp::Eq, Some(json!(1)))];
    assert!(build_for(DbType::Oracle, &["id"], &bad_filter, &[], 1).is_err());
    let bad_order = [OrderBy {
        column: "a;b".into(),
        descending: false,
    }];
    assert!(build_for(DbType::Oracle, &["id"], &[], &bad_order, 1).is_err());
}

#[test]
fn bad_filter_values_are_rejected() {
    let cases = [
        filter("a", FilterOp::Eq, None),
        filter("a", FilterOp::Eq, Some(json!([1]))),
        filter("a", FilterOp::Eq, Some(json!({"x": 1}))),
        filter("a", FilterOp::Eq, Some(json!(null))),
        filter("a", FilterOp::In, Some(json!("x"))),
        filter("a", FilterOp::In, Some(json!([]))),
        filter("a", FilterOp::Eq, Some(json!("nul\u{0}byte"))),
        filter("a", FilterOp::Eq, Some(json!("x".repeat(5_000)))),
    ];
    for case in cases {
        assert!(
            build_for(DbType::Oracle, &["id"], std::slice::from_ref(&case), &[], 1).is_err(),
            "{case:?}"
        );
    }
}

#[test]
fn too_many_filters_or_in_values_are_rejected() {
    let many: Vec<Filter> = (0..21)
        .map(|_| filter("a", FilterOp::Eq, Some(json!(1))))
        .collect();
    assert!(build_for(DbType::Oracle, &["id"], &many, &[], 1).is_err());

    let values: Vec<i32> = (0..101).collect();
    let big_in = [filter("a", FilterOp::In, Some(json!(values)))];
    assert!(build_for(DbType::Oracle, &["id"], &big_in, &[], 1).is_err());
}

#[test]
fn an_empty_column_list_is_rejected() {
    assert!(build_for(DbType::Oracle, &[], &[], &[], 1).is_err());
}
