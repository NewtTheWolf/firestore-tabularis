//! `FilterExpr` validation against Firestore's compound-filter restrictions,
//! and the builder that maps `FilterExpr` → `firestore::FirestoreQueryFilter`.
//!
//! Validation runs before any Firestore call so the user gets a clear error
//! ("inequality on at most one field per query") instead of Firestore's
//! cryptic gRPC message.

use crate::query_parser::{CmpOp, FilterExpr, Literal};
use firestore::{
    FirestoreQueryFilter, FirestoreQueryFilterCompare, FirestoreQueryFilterComposite,
    FirestoreQueryFilterCompositeOperator, FirestoreValue,
};
use std::collections::BTreeSet;

pub fn validate(expr: &FilterExpr) -> Result<(), String> {
    let mut state = ValidationState::default();
    walk(expr, &mut state);

    if state.inequality_fields.len() > 1 {
        let mut fields: Vec<String> = state.inequality_fields.iter().cloned().collect();
        fields.sort();
        return Err(format!(
            "Firestore allows inequality on at most one field per query (saw {}). \
             Adjust the filter or split into multiple queries.",
            fields.join(", ")
        ));
    }

    for (n, kind) in [
        (state.in_max_size, "IN/NOT IN"),
        (state.array_contains_any_max_size, "ARRAY_CONTAINS_ANY"),
    ] {
        if n > 30 {
            return Err(format!(
                "Firestore limits {kind} to 30 values per query (saw {n})."
            ));
        }
    }

    if state.has_array_contains && state.has_array_contains_any {
        return Err(
            "Firestore disallows ARRAY_CONTAINS and ARRAY_CONTAINS_ANY in the same query.".into(),
        );
    }

    if state.array_contains_fields.values().any(|n| *n > 1) {
        let mut fields: Vec<String> = state
            .array_contains_fields
            .iter()
            .filter(|(_, n)| **n > 1)
            .map(|(f, _)| f.clone())
            .collect();
        fields.sort();
        return Err(format!(
            "Firestore allows at most one ARRAY_CONTAINS per field (saw multiple on: {}).",
            fields.join(", ")
        ));
    }

    Ok(())
}

#[derive(Default)]
struct ValidationState {
    inequality_fields: BTreeSet<String>,
    in_max_size: usize,
    array_contains_any_max_size: usize,
    has_array_contains: bool,
    has_array_contains_any: bool,
    array_contains_fields: std::collections::BTreeMap<String, usize>,
}

fn walk(expr: &FilterExpr, state: &mut ValidationState) {
    match expr {
        FilterExpr::Compare { field, op, .. } => {
            if matches!(
                op,
                CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge | CmpOp::Ne
            ) {
                state.inequality_fields.insert(field.join("."));
            }
        }
        FilterExpr::In { values, .. } => {
            state.in_max_size = state.in_max_size.max(values.len());
        }
        FilterExpr::ArrayContains { field, .. } => {
            state.has_array_contains = true;
            *state
                .array_contains_fields
                .entry(field.join("."))
                .or_insert(0) += 1;
        }
        FilterExpr::ArrayContainsAny { values, .. } => {
            state.has_array_contains_any = true;
            state.array_contains_any_max_size = state.array_contains_any_max_size.max(values.len());
        }
        FilterExpr::And(children) | FilterExpr::Or(children) => {
            for c in children {
                walk(c, state);
            }
        }
    }
}

/// Rewrite the synthetic doc-ID column (`id`) to Firestore's wire-level
/// `__name__` field, with the literal value wrapped as a full document
/// resource path. Firestore rejects any user-provided field path beginning
/// with `__`, so we cannot pass the synthetic column through unchanged.
/// Range / IN queries on the doc ID translate the same way.
pub fn rewrite_doc_id(expr: &mut FilterExpr, table: &str, project: &str, database: &str) {
    let prefix = format!("projects/{project}/databases/{database}/documents/{table}/");
    rewrite_walk(expr, &prefix);
}

fn rewrite_walk(expr: &mut FilterExpr, prefix: &str) {
    match expr {
        FilterExpr::Compare { field, value, .. } if is_doc_id(field) => {
            *field = vec!["__name__".to_string()];
            promote_to_reference(value, prefix);
        }
        FilterExpr::In { field, values, .. } if is_doc_id(field) => {
            *field = vec!["__name__".to_string()];
            for v in values {
                promote_to_reference(v, prefix);
            }
        }
        FilterExpr::And(children) | FilterExpr::Or(children) => {
            for c in children {
                rewrite_walk(c, prefix);
            }
        }
        _ => {}
    }
}

fn is_doc_id(field: &[String]) -> bool {
    field.len() == 1 && field[0] == crate::schema_infer::ID_COLUMN
}

fn promote_to_reference(lit: &mut Literal, prefix: &str) {
    if let Literal::Str(s) = lit {
        let path = format!("{prefix}{s}");
        *lit = Literal::Reference(path);
    }
}

/// Convert a Phase-2 `FilterExpr` AST into the firestore-rs filter type.
/// Pre-flight validation should have run already; this function trusts the input.
pub fn build_filter(expr: &FilterExpr) -> FirestoreQueryFilter {
    match expr {
        FilterExpr::Compare { field, op, value } => FirestoreQueryFilter::Compare(Some(
            compare_op(field.join("."), *op, literal_to_firestore_value(value)),
        )),
        FilterExpr::In {
            field,
            values,
            negated,
        } => {
            let arr = literals_to_array(values);
            let cmp = if *negated {
                FirestoreQueryFilterCompare::NotIn(field.join("."), arr)
            } else {
                FirestoreQueryFilterCompare::In(field.join("."), arr)
            };
            FirestoreQueryFilter::Compare(Some(cmp))
        }
        FilterExpr::ArrayContains { field, value } => {
            FirestoreQueryFilter::Compare(Some(FirestoreQueryFilterCompare::ArrayContains(
                field.join("."),
                literal_to_firestore_value(value),
            )))
        }
        FilterExpr::ArrayContainsAny { field, values } => {
            FirestoreQueryFilter::Compare(Some(FirestoreQueryFilterCompare::ArrayContainsAny(
                field.join("."),
                literals_to_array(values),
            )))
        }
        FilterExpr::And(children) => {
            FirestoreQueryFilter::Composite(FirestoreQueryFilterComposite::new(
                children.iter().map(build_filter).collect(),
                FirestoreQueryFilterCompositeOperator::And,
            ))
        }
        FilterExpr::Or(children) => {
            FirestoreQueryFilter::Composite(FirestoreQueryFilterComposite::new(
                children.iter().map(build_filter).collect(),
                FirestoreQueryFilterCompositeOperator::Or,
            ))
        }
    }
}

fn compare_op(path: String, op: CmpOp, value: FirestoreValue) -> FirestoreQueryFilterCompare {
    match op {
        CmpOp::Eq => FirestoreQueryFilterCompare::Equal(path, value),
        CmpOp::Ne => FirestoreQueryFilterCompare::NotEqual(path, value),
        CmpOp::Lt => FirestoreQueryFilterCompare::LessThan(path, value),
        CmpOp::Le => FirestoreQueryFilterCompare::LessThanOrEqual(path, value),
        CmpOp::Gt => FirestoreQueryFilterCompare::GreaterThan(path, value),
        CmpOp::Ge => FirestoreQueryFilterCompare::GreaterThanOrEqual(path, value),
    }
}

fn literal_to_firestore_value(lit: &Literal) -> FirestoreValue {
    use gcloud_sdk::google::firestore::v1::value::ValueType as V;
    use gcloud_sdk::google::firestore::v1::Value as PV;

    let value_type = match lit {
        Literal::Str(s) => V::StringValue(s.clone()),
        Literal::Int(n) => V::IntegerValue(*n),
        Literal::Float(f) => V::DoubleValue(*f),
        Literal::Bool(b) => V::BooleanValue(*b),
        Literal::Null => V::NullValue(0),
        Literal::Timestamp(dt) => V::TimestampValue(gcloud_sdk::prost_types::Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        }),
        Literal::Reference(path) => V::ReferenceValue(path.clone()),
    };
    FirestoreValue::from(PV {
        value_type: Some(value_type),
    })
}

fn literals_to_array(lits: &[Literal]) -> FirestoreValue {
    use gcloud_sdk::google::firestore::v1::{value::ValueType, ArrayValue, Value};
    FirestoreValue::from(Value {
        value_type: Some(ValueType::ArrayValue(ArrayValue {
            values: lits
                .iter()
                .map(|l| literal_to_firestore_value(l).value)
                .collect(),
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_parser::{CmpOp, FilterExpr, Literal};

    fn cmp(field: &[&str], op: CmpOp, lit: Literal) -> FilterExpr {
        FilterExpr::Compare {
            field: field.iter().map(|s| s.to_string()).collect(),
            op,
            value: lit,
        }
    }

    #[test]
    fn validates_inequality_on_one_field() {
        let expr = FilterExpr::And(vec![
            cmp(&["age"], CmpOp::Gt, Literal::Int(18)),
            cmp(&["age"], CmpOp::Lt, Literal::Int(99)),
        ]);
        assert!(validate(&expr).is_ok());
    }

    #[test]
    fn rejects_inequality_on_two_fields() {
        let expr = FilterExpr::And(vec![
            cmp(&["age"], CmpOp::Gt, Literal::Int(18)),
            cmp(&["score"], CmpOp::Lt, Literal::Int(100)),
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("inequality on at most one field"));
        assert!(err.contains("age"));
        assert!(err.contains("score"));
    }

    #[test]
    fn rejects_in_with_31_values() {
        let values: Vec<Literal> = (0..31).map(Literal::Int).collect();
        let expr = FilterExpr::In {
            field: vec!["x".to_string()],
            values,
            negated: false,
        };
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("IN/NOT IN"));
        assert!(err.contains("30"));
        assert!(err.contains("31"));
    }

    #[test]
    fn rejects_array_contains_with_array_contains_any() {
        let expr = FilterExpr::And(vec![
            FilterExpr::ArrayContains {
                field: vec!["tags".to_string()],
                value: Literal::Str("a".into()),
            },
            FilterExpr::ArrayContainsAny {
                field: vec!["tags".to_string()],
                values: vec![Literal::Str("b".into())],
            },
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("ARRAY_CONTAINS"));
        assert!(err.contains("ARRAY_CONTAINS_ANY"));
    }

    #[test]
    fn rejects_two_array_contains_on_same_field() {
        let expr = FilterExpr::And(vec![
            FilterExpr::ArrayContains {
                field: vec!["tags".to_string()],
                value: Literal::Str("a".into()),
            },
            FilterExpr::ArrayContains {
                field: vec!["tags".to_string()],
                value: Literal::Str("b".into()),
            },
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("at most one ARRAY_CONTAINS"));
        assert!(err.contains("tags"));
    }

    #[test]
    fn or_branches_aggregate_for_validation() {
        // Even across OR branches, two distinct inequality fields are still rejected
        // (Firestore restriction holds regardless of conjunction).
        let expr = FilterExpr::Or(vec![
            cmp(&["age"], CmpOp::Gt, Literal::Int(18)),
            cmp(&["score"], CmpOp::Lt, Literal::Int(100)),
        ]);
        let err = validate(&expr).unwrap_err();
        assert!(err.contains("inequality"));
    }

    #[test]
    fn rewrite_doc_id_translates_eq_to_name_with_full_path() {
        let mut expr = FilterExpr::Compare {
            field: vec!["id".to_string()],
            op: CmpOp::Eq,
            value: Literal::Str("callservice".to_string()),
        };
        rewrite_doc_id(&mut expr, "_config", "luninora", "(default)");
        match expr {
            FilterExpr::Compare { field, value, .. } => {
                assert_eq!(field, vec!["__name__".to_string()]);
                assert_eq!(
                    value,
                    Literal::Reference(
                        "projects/luninora/databases/(default)/documents/_config/callservice"
                            .to_string()
                    )
                );
            }
            _ => panic!("expected Compare, got {expr:?}"),
        }
    }

    #[test]
    fn rewrite_doc_id_recurses_into_and() {
        let mut expr = FilterExpr::And(vec![
            cmp(&["id"], CmpOp::Eq, Literal::Str("a".into())),
            cmp(&["status"], CmpOp::Eq, Literal::Str("active".into())),
        ]);
        rewrite_doc_id(&mut expr, "users", "p1", "(default)");
        match expr {
            FilterExpr::And(terms) => {
                match &terms[0] {
                    FilterExpr::Compare { field, value, .. } => {
                        assert_eq!(field, &vec!["__name__".to_string()]);
                        assert!(matches!(value, Literal::Reference(_)));
                    }
                    other => panic!("expected rewritten Compare, got {other:?}"),
                }
                match &terms[1] {
                    FilterExpr::Compare { field, value, .. } => {
                        assert_eq!(field, &vec!["status".to_string()]);
                        assert_eq!(value, &Literal::Str("active".into()));
                    }
                    other => panic!("expected untouched Compare, got {other:?}"),
                }
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rewrite_doc_id_translates_in_list() {
        let mut expr = FilterExpr::In {
            field: vec!["id".to_string()],
            values: vec![Literal::Str("a".into()), Literal::Str("b".into())],
            negated: false,
        };
        rewrite_doc_id(&mut expr, "users", "p1", "(default)");
        match expr {
            FilterExpr::In { field, values, .. } => {
                assert_eq!(field, vec!["__name__".to_string()]);
                assert_eq!(values.len(), 2);
                assert!(matches!(values[0], Literal::Reference(_)));
                assert!(matches!(values[1], Literal::Reference(_)));
            }
            _ => panic!(),
        }
    }

    #[test]
    fn rewrite_doc_id_leaves_other_fields_alone() {
        let mut expr = cmp(&["status"], CmpOp::Eq, Literal::Str("active".into()));
        let before = expr.clone();
        rewrite_doc_id(&mut expr, "users", "p1", "(default)");
        assert_eq!(expr, before);
    }
}
