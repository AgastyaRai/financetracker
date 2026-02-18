-- create sessions table
CREATE TABLE IF NOT EXISTS sessions (
  id uuid PRIMARY KEY,
  user_id uuid NOT NULL REFERENCES users(id) ON DELETE CASCADE, -- links to users
  created_at timestamptz NOT NULL DEFAULT now(),
  expires_at timestamptz NOT NULL
);