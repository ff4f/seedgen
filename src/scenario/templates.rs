use crate::scenario::parser::{parse_scenario, ScenarioConfig, ScenarioError};

const ECOMMERCE: &str = include_str!("../../scenarios/ecommerce.yaml");
const SAAS: &str = include_str!("../../scenarios/saas.yaml");
const BLOG: &str = include_str!("../../scenarios/blog.yaml");
const SOCIAL: &str = include_str!("../../scenarios/social.yaml");

pub const TEMPLATE_NAMES: &[&str] = &["ecommerce", "saas", "blog", "social"];

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("unknown scenario template `{name}` (available: {})", available.join(", "))]
    Unknown {
        name: String,
        available: Vec<&'static str>,
    },

    #[error("built-in template `{name}` failed to parse: {source}")]
    Malformed {
        name: String,
        #[source]
        source: ScenarioError,
    },
}

pub fn load_template(name: &str) -> Result<ScenarioConfig, TemplateError> {
    let yaml = match name {
        "ecommerce" => ECOMMERCE,
        "saas" => SAAS,
        "blog" => BLOG,
        "social" => SOCIAL,
        other => {
            return Err(TemplateError::Unknown {
                name: other.to_string(),
                available: TEMPLATE_NAMES.to_vec(),
            });
        }
    };
    parse_scenario(yaml).map_err(|source| TemplateError::Malformed {
        name: name.to_string(),
        source,
    })
}

pub fn list_templates() -> &'static [&'static str] {
    TEMPLATE_NAMES
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::parser::{ColumnOverride, CountExpression};

    #[test]
    fn test_list_templates() {
        let names = list_templates();
        assert_eq!(names.len(), 4);
        assert!(names.contains(&"ecommerce"));
        assert!(names.contains(&"saas"));
        assert!(names.contains(&"blog"));
        assert!(names.contains(&"social"));
    }

    #[test]
    fn test_unknown_template_lists_available() {
        let err = load_template("nope").unwrap_err();
        match err {
            TemplateError::Unknown { name, available } => {
                assert_eq!(name, "nope");
                assert_eq!(available.len(), 4);
            }
            other => panic!("got {other:?}"),
        }
    }

    fn fixed(t: &crate::scenario::parser::TableScenario) -> usize {
        match t.count {
            CountExpression::Fixed(n) => n,
            ref other => panic!("expected Fixed, got {other:?}"),
        }
    }

    #[test]
    fn test_ecommerce_loads_and_has_expected_tables() {
        let s = load_template("ecommerce").expect("ecommerce should parse");
        for t in [
            "users",
            "categories",
            "products",
            "orders",
            "order_items",
            "reviews",
        ] {
            assert!(s.tables.contains_key(t), "missing table `{t}`");
        }
        assert_eq!(fixed(&s.tables["users"]), 50);
        assert_eq!(fixed(&s.tables["categories"]), 15);
        assert_eq!(fixed(&s.tables["products"]), 200);
    }

    #[test]
    fn test_ecommerce_order_status_distribution_sums_to_100() {
        let s = load_template("ecommerce").unwrap();
        let status = s.tables["orders"]
            .overrides
            .get("status")
            .expect("status override missing");
        let total: f64 = match status {
            ColumnOverride::Distribution(d) => d.values().sum(),
            other => panic!("got {other:?}"),
        };
        assert!((total - 100.0).abs() < 1e-9, "got {total}");
    }

    #[test]
    fn test_saas_loads_and_has_expected_tables() {
        let s = load_template("saas").expect("saas should parse");
        for t in ["organizations", "users", "subscriptions", "invoices"] {
            assert!(s.tables.contains_key(t), "missing table `{t}`");
        }
        assert_eq!(fixed(&s.tables["organizations"]), 10);
    }

    #[test]
    fn test_saas_subscription_plan_distribution_present() {
        let s = load_template("saas").unwrap();
        let plan = s.tables["subscriptions"]
            .overrides
            .get("plan")
            .expect("plan override missing");
        match plan {
            ColumnOverride::Distribution(d) => {
                assert!(d.contains_key("free"));
                assert!(d.contains_key("pro"));
                assert!(d.contains_key("enterprise"));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_blog_loads_and_has_expected_tables() {
        let s = load_template("blog").expect("blog should parse");
        for t in ["users", "posts", "comments", "tags", "post_tags"] {
            assert!(s.tables.contains_key(t), "missing table `{t}`");
        }
        assert_eq!(fixed(&s.tables["users"]), 20);
        assert_eq!(fixed(&s.tables["tags"]), 30);
    }

    #[test]
    fn test_blog_user_role_distribution_sums_to_100() {
        let s = load_template("blog").unwrap();
        let role = s.tables["users"]
            .overrides
            .get("role")
            .expect("role override missing");
        let total: f64 = match role {
            ColumnOverride::Distribution(d) => d.values().sum(),
            other => panic!("got {other:?}"),
        };
        assert!((total - 100.0).abs() < 1e-9, "got {total}");
    }

    #[test]
    fn test_social_loads_and_has_expected_tables() {
        let s = load_template("social").expect("social should parse");
        for t in ["users", "posts", "likes", "follows", "messages"] {
            assert!(s.tables.contains_key(t), "missing table `{t}`");
        }
        assert_eq!(fixed(&s.tables["users"]), 100);
    }

    #[test]
    fn test_social_follows_is_percentage_of_users() {
        let s = load_template("social").unwrap();
        match &s.tables["follows"].count {
            CountExpression::PercentageOf { table, percentage } => {
                assert_eq!(table, "users");
                assert!((*percentage - 30.0).abs() < 1e-9);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn test_all_templates_parse() {
        for name in list_templates() {
            load_template(name).unwrap_or_else(|e| panic!("`{name}` failed: {e}"));
        }
    }
}
