CREATE TABLE users (
    id         SERIAL PRIMARY KEY,
    email      VARCHAR UNIQUE NOT NULL,
    name       VARCHAR NOT NULL,
    bio        TEXT,
    is_active  BOOLEAN DEFAULT true,
    created_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE posts (
    id           SERIAL PRIMARY KEY,
    user_id      INTEGER NOT NULL REFERENCES users(id),
    title        VARCHAR(200) NOT NULL,
    slug         VARCHAR(200) UNIQUE NOT NULL,
    body         TEXT,
    published_at TIMESTAMP
);

CREATE TABLE comments (
    id         SERIAL PRIMARY KEY,
    post_id    INTEGER NOT NULL REFERENCES posts(id),
    user_id    INTEGER NOT NULL REFERENCES users(id),
    content    TEXT NOT NULL,
    created_at TIMESTAMP DEFAULT NOW()
);
