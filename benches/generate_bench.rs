use std::fs;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use sqlx::PgPool;

use seedgen::generators::{create_generator, Generator, Value};
use seedgen::introspection::{introspect, Column, DataType, ForeignKey, SchemaGraph, Table};
use seedgen::resolver::topological_sort;
use seedgen::semantic::{detect_generator, GeneratorType};

// ---------- fixtures (no DB) ------------------------------------------------

fn basic_users_schema() -> SchemaGraph {
    // Mirrors tests/fixtures/schema_basic.sql users table — same columns,
    // same types, same nullability — so semantic detection picks the same
    // generators as the live-DB path.
    SchemaGraph {
        tables: vec![Table {
            name: "users".into(),
            columns: vec![
                col(
                    "id",
                    DataType::Integer,
                    false,
                    true,
                    Some("nextval('users_id_seq'::regclass)"),
                ),
                col("email", DataType::Varchar, false, false, None),
                col("name", DataType::Varchar, false, false, None),
                col("bio", DataType::Text, true, false, None),
                col("is_active", DataType::Boolean, true, false, Some("true")),
                col(
                    "created_at",
                    DataType::Timestamp,
                    true,
                    false,
                    Some("now()"),
                ),
            ],
            constraints: vec![],
        }],
        foreign_keys: vec![],
        enums: vec![],
    }
}

fn col(
    name: &str,
    data_type: DataType,
    is_nullable: bool,
    is_identity: bool,
    default_value: Option<&str>,
) -> Column {
    Column {
        name: name.to_string(),
        data_type,
        is_nullable,
        is_identity,
        is_generated: false,
        default_value: default_value.map(String::from),
        max_length: None,
        numeric_precision: None,
        numeric_scale: None,
    }
}

fn build_generators(schema: &SchemaGraph) -> Vec<Box<dyn Generator>> {
    let table = &schema.tables[0];
    let mut out = Vec::new();
    for column in &table.columns {
        if column.is_generated || column.is_identity {
            continue;
        }
        let gt = detect_generator(column, &schema.enums);
        if matches!(gt, GeneratorType::Skip) {
            continue;
        }
        out.push(create_generator(&gt));
    }
    out
}

fn generate_in_memory(gens: &[Box<dyn Generator>], rows: usize, rng: &mut ChaCha8Rng) -> usize {
    // Generates and immediately drops each row. We return the count so the
    // optimizer can't strip the whole loop. `black_box` on the result also helps.
    let mut count = 0;
    for _ in 0..rows {
        let row: Vec<Value> = gens.iter().map(|g| g.generate(rng)).collect();
        count += row.len();
    }
    count
}

fn synthetic_50_table_schema() -> (Vec<String>, Vec<ForeignKey>) {
    let tables: Vec<String> = (0..50).map(|i| format!("t_{i:02}")).collect();
    let mut fks = Vec::with_capacity(100);
    let mut rng = ChaCha8Rng::seed_from_u64(0xBEEF);
    // 100 FKs from child→parent where parent_idx < child_idx (DAG, no cycles).
    while fks.len() < 100 {
        let child = rng.gen_range(1..50);
        let parent = rng.gen_range(0..child);
        fks.push(ForeignKey {
            from_table: tables[child].clone(),
            from_column: format!("c{}", fks.len()),
            to_table: tables[parent].clone(),
            to_column: "id".into(),
            is_nullable: rng.gen_bool(0.3),
            is_deferrable: false,
        });
    }
    (tables, fks)
}

// ---------- benchmarks ------------------------------------------------------

fn bench_introspect(c: &mut Criterion) {
    let url = match std::env::var("DATABASE_URL") {
        Ok(u) => u,
        Err(_) => {
            eprintln!("⚠ DATABASE_URL not set; skipping bench_introspect");
            return;
        }
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let pool: PgPool = rt.block_on(async {
        let pool = PgPool::connect(&url).await.expect("connect");
        sqlx::raw_sql("DROP SCHEMA IF EXISTS public CASCADE; CREATE SCHEMA public;")
            .execute(&pool)
            .await
            .expect("reset schema");
        let fixture =
            fs::read_to_string("tests/fixtures/schema_basic.sql").expect("read schema_basic.sql");
        sqlx::raw_sql(&fixture)
            .execute(&pool)
            .await
            .expect("apply schema_basic.sql");
        pool
    });

    c.bench_function("introspect", |b| {
        b.to_async(&rt).iter(|| async {
            let _ = introspect(black_box(&pool)).await.expect("introspect");
        });
    });
}

fn bench_generate_100_rows(c: &mut Criterion) {
    let schema = basic_users_schema();
    let gens = build_generators(&schema);
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    c.bench_function("generate_100_rows", |b| {
        b.iter(|| {
            let n = generate_in_memory(black_box(&gens), 100, &mut rng);
            black_box(n);
        });
    });
}

fn bench_generate_1000_rows(c: &mut Criterion) {
    let schema = basic_users_schema();
    let gens = build_generators(&schema);
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    c.bench_function("generate_1000_rows", |b| {
        b.iter(|| {
            let n = generate_in_memory(black_box(&gens), 1000, &mut rng);
            black_box(n);
        });
    });
}

fn bench_generate_10000_rows(c: &mut Criterion) {
    let schema = basic_users_schema();
    let gens = build_generators(&schema);
    let mut rng = ChaCha8Rng::seed_from_u64(42);
    c.bench_function("generate_10000_rows", |b| {
        b.iter(|| {
            let n = generate_in_memory(black_box(&gens), 10_000, &mut rng);
            black_box(n);
        });
    });
}

fn bench_topological_sort_50_tables(c: &mut Criterion) {
    let (tables, fks) = synthetic_50_table_schema();
    c.bench_function("topological_sort_50_tables", |b| {
        b.iter(|| {
            let _ =
                topological_sort(black_box(&tables), black_box(&fks)).expect("topological sort");
        });
    });
}

criterion_group!(
    benches,
    bench_introspect,
    bench_generate_100_rows,
    bench_generate_1000_rows,
    bench_generate_10000_rows,
    bench_topological_sort_50_tables,
);
criterion_main!(benches);
