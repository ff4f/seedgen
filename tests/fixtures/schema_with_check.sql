-- Fixture for property tests that need a CHECK constraint.
-- Uses DOUBLE PRECISION instead of NUMERIC because sqlx requires the bigdecimal
-- feature to bind NUMERIC, and we want this fixture to work with the default
-- sqlx features. The CHECK semantics are the same: enforce price > 0.

CREATE TABLE priced_items (
    id    SERIAL PRIMARY KEY,
    name  VARCHAR(120) NOT NULL,
    price DOUBLE PRECISION NOT NULL CHECK (price > 0)
);
