"""Optional connector checks, including real SQLite and PostgreSQL boundaries."""

import copy
from contextlib import closing
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest
import uuid

import sqlalchemy


SCRIPT = Path(__file__).resolve().parents[1] / "tools/sql/read/read.py"
spec = importlib.util.spec_from_file_location("sql_read", SCRIPT)
reader = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reader)


def config(url, dialect="sqlite"):
    return {
        "source": "analytics:test", "url": url, "dialect": dialect,
        "read_only": True, "max_rows": 2,
        "queries": {
            "campaigns": {
                "description": "Rank qualified trials by campaign.",
                "sql": "SELECT campaign, count(*) AS trials FROM trials WHERE qualified_at >= :since GROUP BY campaign ORDER BY trials DESC, campaign",
                "parameters_schema": {
                    "type": "object", "additionalProperties": False,
                    "required": ["since"],
                    "properties": {"since": {"type": "string", "maxLength": 128}},
                },
                "result_description": "A qualified trial count, not causal attribution.",
            },
        },
    }


def query(**changes):
    return {"action": "query", "source": "analytics:test", "query_id": "campaigns",
            "parameters": {"since": "2026-10-01"}, "max_rows": 2, **changes}


class SQLReadTest(unittest.TestCase):
    def setUp(self):
        self.area = tempfile.TemporaryDirectory()
        self.path = Path(self.area.name) / "analytics.db"
        with closing(sqlite3.connect(self.path)) as connection, connection:
            connection.execute("CREATE TABLE trials(campaign TEXT, qualified_at TEXT)")
            connection.executemany("INSERT INTO trials VALUES (?, ?)", [
                ("launch", "2026-10-02"), ("launch", "2026-10-03"),
                ("docs", "2026-10-05"), ("search", "2026-10-06"),
                ("old", "2026-09-01"),
            ])
        self.config = config(f"sqlite:///file:{self.path}?mode=ro&uri=true")

    def tearDown(self):
        self.area.cleanup()

    def test_real_read_and_explicit_truncation(self):
        result = reader.read(query(), self.config)
        self.assertEqual(result["columns"], ["campaign", "trials"])
        self.assertEqual(result["rows"], [["launch", 2], ["docs", 1]])
        self.assertTrue(result["executed"])
        self.assertTrue(result["truncated"])
        smaller = reader.read(query(max_rows=1), self.config)
        self.assertEqual(len(smaller["rows"]), 1)
        with closing(sqlite3.connect(self.path)) as connection:
            self.assertEqual(connection.execute("SELECT count(*) FROM trials").fetchone()[0], 5)

    def test_bound_values_cannot_change_sql(self):
        result = reader.read(query(parameters={"since": "x'; DROP TABLE trials; --"}), self.config)
        self.assertEqual(result["rows"], [])
        with closing(sqlite3.connect(self.path)) as connection:
            self.assertEqual(connection.execute("SELECT count(*) FROM trials").fetchone()[0], 5)

    def test_catalog_opens_no_database_and_hides_sql_and_secrets(self):
        self.config["url"] = "missingdriver://secret:password@invalid/db"
        result = reader.read({"action": "catalog", "source": "analytics:test"}, self.config)
        encoded = json.dumps(result)
        self.assertFalse(result["executed"])
        self.assertEqual(result["queries"][0]["id"], "campaigns")
        for secret in ["password", "missingdriver", "SELECT"]:
            self.assertNotIn(secret, encoded)

    def test_invalid_input_never_opens_a_connection(self):
        self.config["url"] = "missingdriver://invalid"
        for inputs in [query(parameters={}), query(parameters={"since": None}),
                       query(parameters={"since": "x", "extra": 1}),
                       query(query_id="unknown"), query(source="another:source"),
                       query(sql="DELETE FROM trials")]:
            with self.subTest(inputs=inputs), self.assertRaises(reader.ReadError) as raised:
                reader.read(inputs, self.config)
            self.assertNotEqual(str(raised.exception), "sql.database_read_failed")

    def test_full_parameter_schema_and_external_ref_refusal(self):
        schema = self.config["queries"]["campaigns"]["parameters_schema"]
        schema["properties"]["since"] = {"anyOf": [
            {"type": "string", "pattern": "^2026-10-01$"}, {"const": "2026-10-02"}
        ]}
        with self.assertRaisesRegex(reader.ReadError, "sql.invalid_parameters"):
            reader.read(query(parameters={"since": "invalid"}), self.config)
        schema["properties"]["since"] = {"$ref": "https://invalid.example/schema"}
        with self.assertRaisesRegex(reader.ReadError, "sql.invalid_parameters"):
            reader.read(query(), self.config)

    def test_every_sql_bind_must_be_required_before_opening_a_connection(self):
        self.config["url"] = "missingdriver://must-not-open"
        self.config["queries"]["campaigns"]["parameters_schema"].pop("required")
        with self.assertRaisesRegex(reader.ReadError, "sql.query_configuration_invalid"):
            reader.read(query(parameters={}), self.config)

    def test_anchored_parameter_patterns_use_the_native_ecma_semantics(self):
        self.config["url"] = "missingdriver://must-not-open"
        schema = self.config["queries"]["campaigns"]["parameters_schema"]
        schema["properties"]["since"] = {"type": "string", "pattern": "^2026-10-[0-9]{2}$"}
        with self.assertRaisesRegex(reader.ReadError, "sql.invalid_parameters"):
            reader.read(query(parameters={"since": "2026-10-01\n"}), self.config)

    def test_schema_patterns_cannot_introduce_unused_sql_parameters(self):
        self.config["url"] = "missingdriver://must-not-open"
        schema = self.config["queries"]["campaigns"]["parameters_schema"]
        schema["patternProperties"] = {"^extra_": {"type": "string"}}
        with self.assertRaisesRegex(reader.ReadError, "sql.invalid_parameters"):
            reader.read(query(parameters={"since": "2026-10-01", "extra_filter": "ignored"}), self.config)

    def test_mutating_declared_statements_are_rejected(self):
        for sql in ["DELETE FROM trials", "SELECT 1; DELETE FROM trials", "SELECT 1 INTO other",
                    "SELECT * FROM trials FOR UPDATE",
                    "WITH removed AS (DELETE FROM trials RETURNING *) SELECT * FROM removed"]:
            candidate = copy.deepcopy(self.config["queries"]["campaigns"])
            candidate["sql"] = sql
            candidate["parameters_schema"] = {"type": "object", "additionalProperties": False}
            with self.subTest(sql=sql), self.assertRaises(reader.ReadError):
                reader.declared_query(candidate, "postgres")

    def test_sqlite_read_only_binding_rejects_writes_independently_of_parser(self):
        engine = sqlalchemy.create_engine(self.config["url"])
        try:
            with engine.connect() as connection, self.assertRaises(sqlalchemy.exc.DBAPIError):
                connection.execute(sqlalchemy.text("DELETE FROM trials"))
        finally:
            engine.dispose()

    def test_byte_limit_fails_instead_of_silently_dropping_rows(self):
        candidate = self.config["queries"]["campaigns"]
        candidate["sql"] = "SELECT :since || hex(zeroblob(1000)) AS large_value"
        self.config["max_bytes"] = 1024
        with self.assertRaisesRegex(reader.ReadError, "sql.result_byte_limit"):
            reader.read(query(), self.config)

    def test_process_errors_do_not_expose_credentials_or_driver_exceptions(self):
        self.config["url"] = "unknown://private_user:PRIVATE_PASSWORD@invalid/db"
        env = {**os.environ, "RUNX_SQL_CONNECTION": json.dumps(self.config)}
        result = subprocess.run([sys.executable, str(SCRIPT)], input=json.dumps(query()),
                                text=True, capture_output=True, env=env)
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertEqual(result.stderr.strip(), "sql.database_read_failed")

    def test_exact_numeric_cells_and_unsupported_types(self):
        import decimal
        self.assertEqual(reader.cell(9007199254740993), {"$type": "integer", "value": "9007199254740993"})
        self.assertEqual(reader.cell(decimal.Decimal("1.2300")), {"$type": "decimal", "value": "1.2300"})
        with self.assertRaises(reader.ReadError):
            reader.cell(float("nan"))

    @unittest.skipUnless(os.environ.get("RUNX_SQL_TEST_POSTGRES_URL"), "disposable PostgreSQL not supplied")
    def test_postgres_real_read_and_database_enforced_read_only_role(self):
        from sqlalchemy.engine import make_url
        suffix = uuid.uuid4().hex[:12]
        role, schema, password = "reader_" + suffix, "read_test_" + suffix, uuid.uuid4().hex
        owner_url = make_url(os.environ["RUNX_SQL_TEST_POSTGRES_URL"])
        owner = sqlalchemy.create_engine(owner_url)
        try:
            with owner.begin() as connection:
                connection.exec_driver_sql(f'CREATE ROLE "{role}" LOGIN PASSWORD \'{password}\'')
                connection.exec_driver_sql(f'CREATE SCHEMA "{schema}"')
                connection.exec_driver_sql(f'CREATE TABLE "{schema}".trials(campaign text, qualified_at text)')
                connection.exec_driver_sql(f'INSERT INTO "{schema}".trials VALUES (\'launch\', \'2026-10-02\')')
                connection.exec_driver_sql(f'GRANT USAGE ON SCHEMA "{schema}" TO "{role}"')
                connection.exec_driver_sql(f'GRANT SELECT ON ALL TABLES IN SCHEMA "{schema}" TO "{role}"')
            url = owner_url.set(username=role, password=password)
            cfg = config(url.render_as_string(hide_password=False), "postgres")
            cfg["connect_args"] = {"options": f"-c search_path={schema} -c statement_timeout=1000 -c default_transaction_read_only=on"}
            result = reader.read(query(), cfg)
            self.assertEqual(result["rows"], [["launch", 1]])
            self.assertFalse(result["truncated"])
            precise = copy.deepcopy(cfg)
            precise["queries"]["campaigns"]["sql"] = "SELECT CAST(9007199254740993 AS BIGINT), CAST(1.2300 AS NUMERIC(10,4)) WHERE CAST(:since AS TEXT) IS NOT NULL"
            self.assertEqual(reader.read(query(), precise)["rows"], [[
                {"$type": "integer", "value": "9007199254740993"},
                {"$type": "decimal", "value": "1.2300"},
            ]])
            slow = copy.deepcopy(cfg)
            slow["queries"]["campaigns"]["sql"] = "SELECT pg_sleep(10) WHERE CAST(:since AS TEXT) IS NOT NULL"
            with self.assertRaisesRegex(reader.ReadError, "sql.database_read_failed"):
                reader.read(query(), slow)
            restricted = sqlalchemy.create_engine(url, connect_args=cfg["connect_args"])
            try:
                with restricted.connect() as connection, self.assertRaises(sqlalchemy.exc.DBAPIError):
                    connection.execute(sqlalchemy.text("DELETE FROM trials"))
            finally:
                restricted.dispose()
        finally:
            with owner.begin() as connection:
                connection.exec_driver_sql(f'DROP SCHEMA IF EXISTS "{schema}" CASCADE')
                connection.exec_driver_sql(f'DROP ROLE IF EXISTS "{role}"')
            owner.dispose()


if __name__ == "__main__":
    unittest.main()
