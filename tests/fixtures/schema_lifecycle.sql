-- Schema for lifecycle (time-travel) integration tests.

CREATE TABLE users (
    id         SERIAL PRIMARY KEY,
    email      VARCHAR UNIQUE NOT NULL,
    name       VARCHAR NOT NULL,
    is_active  BOOLEAN NOT NULL DEFAULT true,
    churned_at TIMESTAMP,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE orders (
    id           SERIAL PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id),
    status       VARCHAR NOT NULL DEFAULT 'pending',
    total_amount BIGINT,
    created_at   TIMESTAMP NOT NULL DEFAULT NOW()
);

CREATE TABLE order_items (
    id         SERIAL PRIMARY KEY,
    order_id   INTEGER NOT NULL REFERENCES orders(id),
    quantity   INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);
