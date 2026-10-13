-- Supabase security setup is applied separately from SQLx schema migrations.
-- Run the entire file in one transaction as the reviewed postgres backend owner.
-- The Rust API handles custom user/JWT ownership; Data API roles have no application access.
SET LOCAL lock_timeout = '5s';
SET LOCAL statement_timeout = '30s';

DO $$
DECLARE
    private_tables text[] := ARRAY[
        '_sqlx_migrations', 'users', 'transactions', 'budgets', 'sessions',
        'transaction_embeddings', 'bank_connections', 'imported_transaction_details'
    ];
    private_table text;
    vector_schema text;
    vector_type oid;
BEGIN
    -- Refuse a different access model instead of hiding a policy or disrupting a backend role.
    IF current_user <> 'postgres' OR NOT (
        SELECT rolbypassrls FROM pg_roles WHERE rolname = current_user
    ) THEN
        RAISE EXCEPTION 'Security setup requires the reviewed postgres backend owner';
    END IF;
    IF (SELECT count(*) FROM pg_roles WHERE rolname IN ('anon', 'authenticated')) <> 2 THEN
        RAISE EXCEPTION 'Supabase Data API roles must exist before security setup';
    END IF;
    IF NOT ('extensions' = ANY(current_schemas(false))) THEN
        RAISE EXCEPTION 'Backend search_path must include extensions before moving pgvector';
    END IF;
    IF EXISTS (
        SELECT 1 FROM pg_policies
        WHERE schemaname = 'public' AND tablename = ANY(private_tables)
    ) THEN
        RAISE EXCEPTION 'Existing Data API policies require review before security setup';
    END IF;
    FOREACH private_table IN ARRAY private_tables LOOP
        IF NOT EXISTS (
            SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'public' AND c.relname = private_table
                AND c.relkind = 'r' AND pg_get_userbyid(c.relowner) = current_user
        ) THEN
            RAISE EXCEPTION 'Private table public.% is missing or has a different owner', private_table;
        END IF;
    END LOOP;

    -- ALTER EXTENSION checks permission; Supabase authorizes managed extension changes through supautils.
    SELECT n.nspname INTO vector_schema
    FROM pg_extension e JOIN pg_namespace n ON n.oid = e.extnamespace
    WHERE e.extname = 'vector' AND e.extrelocatable;
    IF vector_schema IS NULL OR vector_schema NOT IN ('public', 'extensions') THEN
        RAISE EXCEPTION 'Expected relocatable pgvector in public or extensions';
    END IF;
    SELECT t.oid INTO vector_type FROM pg_type t JOIN pg_namespace n ON n.oid = t.typnamespace
    WHERE n.nspname = vector_schema AND t.typname = 'vector';

    -- Keep RLS as a second boundary, and hide table metadata by revoking the object grants too.
    FOREACH private_table IN ARRAY private_tables LOOP
        EXECUTE format('ALTER TABLE public.%I ENABLE ROW LEVEL SECURITY', private_table);
        EXECUTE format('REVOKE ALL ON TABLE public.%I FROM anon, authenticated, PUBLIC', private_table);
    END LOOP;
    -- Revoking only DML leaves TRUNCATE/REFERENCES/TRIGGER and sequence UPDATE grants behind.
    ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public
        REVOKE ALL ON TABLES FROM anon, authenticated, PUBLIC;
    ALTER DEFAULT PRIVILEGES FOR ROLE postgres IN SCHEMA public
        REVOKE ALL ON SEQUENCES FROM anon, authenticated, PUBLIC;

    -- Relocation retains the type and operator identities used by existing columns and HNSW indexes.
    IF vector_schema = 'public' THEN
        ALTER EXTENSION vector SET SCHEMA extensions;
    END IF;
    IF to_regtype('vector')::oid IS DISTINCT FROM vector_type
        OR to_regtype('extensions.vector')::oid IS DISTINCT FROM vector_type THEN
        RAISE EXCEPTION 'Backend vector type resolution changed; roll back security setup';
    END IF;
END
$$;

NOTIFY pgrst, 'reload schema';
