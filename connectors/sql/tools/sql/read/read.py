"""Trusted SQL provider process. Runx owns admission, secrets and supervision.

Only operator-declared statements are executable; callers supply bound values,
never SQL or connection settings. Database permissions remain authoritative:
parsing a SELECT cannot prove that every database function is side-effect free.
"""

import base64
import datetime
import decimal
import json
import math
import os
import sys
import time

import sqlalchemy
import sqlglot
from jsonschema_rs import Draft202012Validator
from sqlalchemy.pool import NullPool
from sqlglot import exp


MAX_BYTES = 2 * 1024 * 1024
ID = {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9._:/-]{0,191}$"}
QUERY_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": ["description", "sql", "parameters_schema", "result_description"],
    "properties": {
        "description": {"type": "string", "minLength": 1, "maxLength": 4000},
        "sql": {"type": "string", "minLength": 1, "maxLength": 65536},
        "parameters_schema": {"type": "object"},
        "result_description": {"type": "string", "minLength": 1, "maxLength": 4000},
    },
}
CONFIG_SCHEMA = {
    "type": "object",
    "additionalProperties": False,
    "required": ["source_ref", "url", "dialect", "read_only", "queries"],
    "properties": {
        "source_ref": ID,
        "url": {"type": "string", "minLength": 1, "maxLength": 8192},
        "dialect": {"type": "string", "minLength": 1, "maxLength": 64},
        "read_only": {"const": True},
        "connect_args": {"type": "object"},
        "execution_options": {"type": "object"},
        "max_rows": {"type": "integer", "minimum": 1, "maximum": 1000},
        "max_bytes": {"type": "integer", "minimum": 1024, "maximum": MAX_BYTES},
        "queries": {
            "type": "object", "minProperties": 1, "maxProperties": 64,
            "propertyNames": ID, "additionalProperties": QUERY_SCHEMA,
        },
    },
}
INPUT_SCHEMA = {
    "type": "object", "additionalProperties": False,
    "required": ["action", "source_ref"],
    "properties": {
        "action": {"enum": ["catalog", "query"]},
        "source_ref": ID,
        "query_id": ID,
        "parameters": {"type": "object", "maxProperties": 64},
        "max_rows": {"type": "integer", "minimum": 1, "maximum": 1000},
    },
}


class ReadError(Exception):
    """A public error code, never a driver error containing credentials or SQL."""


def validate(schema, value, code):
    try:
        Draft202012Validator(schema, offline=True, validate_formats=False).validate(value)
    except Exception:
        raise ReadError(code) from None


def declared_query(query, dialect):
    try:
        statements = sqlglot.parse(query["sql"], read=dialect)
        if len(statements) != 1 or not isinstance(statements[0], exp.Query):
            raise ValueError()
        # Includes mutations nested in a CTE, SELECT INTO and locking reads.
        if any(isinstance(node, (exp.DML, exp.DDL, exp.Into, exp.Lock, exp.Command))
               for node in statements[0].walk()):
            raise ValueError()
        schema = query["parameters_schema"]
        if schema.get("type") != "object" or schema.get("additionalProperties") is not False:
            raise ValueError()
        Draft202012Validator(schema, offline=True, validate_formats=False)
        statement = sqlalchemy.text(query["sql"])
        binds = set(statement.compile().params)
        if binds != set(schema.get("properties", {})) or binds != set(schema.get("required", [])):
            raise ValueError()
        return statement
    except Exception:
        raise ReadError("sql.query_configuration_invalid") from None


def load_config(raw):
    try:
        if not raw or len(raw.encode()) > 256 * 1024:
            raise ValueError()
        config = json.loads(raw)
    except Exception:
        raise ReadError("sql.connection_configuration_missing_or_invalid") from None
    validate(CONFIG_SCHEMA, config, "sql.connection_configuration_invalid")
    for query in config["queries"].values():
        declared_query(query, config["dialect"])
    return config


def cell(value):
    if value is None or isinstance(value, (str, bool)):
        return value
    if isinstance(value, int):
        return value if abs(value) <= 9007199254740991 else {"$type": "integer", "value": str(value)}
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ReadError("sql.non_finite_number")
        return value
    if isinstance(value, decimal.Decimal):
        return {"$type": "decimal", "value": str(value)}
    if isinstance(value, (datetime.datetime, datetime.date, datetime.time)):
        return {"$type": type(value).__name__, "value": value.isoformat()}
    if isinstance(value, (bytes, bytearray, memoryview)):
        return {"$type": "bytes", "base64": base64.b64encode(value).decode("ascii")}
    if isinstance(value, (list, tuple)):
        return [cell(item) for item in value]
    if isinstance(value, dict) and all(isinstance(key, str) for key in value):
        return {key: cell(item) for key, item in value.items()}
    raise ReadError("sql.unsupported_result_type")


def encode(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False, allow_nan=False)


def read(inputs, config):
    validate(INPUT_SCHEMA, inputs, "sql.invalid_input")
    if inputs["source_ref"] != config["source_ref"]:
        raise ReadError("sql.source_mismatch")
    common = {"source_ref": config["source_ref"], "dialect": config["dialect"]}
    if inputs["action"] == "catalog":
        if set(inputs) != {"action", "source_ref"}:
            raise ReadError("sql.invalid_catalog_input")
        return {
            **common, "status": "catalog", "executed": False,
            "max_rows": config.get("max_rows", 100),
            "queries": [{"id": name, **{key: query[key] for key in (
                "description", "parameters_schema", "result_description"
            )}} for name, query in config["queries"].items()],
        }
    if set(inputs) != {"action", "source_ref", "query_id", "parameters", "max_rows"}:
        raise ReadError("sql.incomplete_query_input")
    query = config["queries"].get(inputs["query_id"])
    if query is None:
        raise ReadError("sql.unknown_query")
    validate(query["parameters_schema"], inputs["parameters"], "sql.invalid_parameters")
    limit = min(inputs["max_rows"], config.get("max_rows", 100))
    maximum_bytes = config.get("max_bytes", MAX_BYTES)
    statement = declared_query(query, config["dialect"])
    if set(inputs["parameters"]) != set(statement.compile().params):
        raise ReadError("sql.invalid_parameters")
    started = time.monotonic()
    engine = None
    try:
        engine = sqlalchemy.create_engine(
            config["url"], connect_args=config.get("connect_args", {}),
            execution_options=config.get("execution_options", {}),
            poolclass=NullPool, hide_parameters=True, echo=False,
        )
        with engine.connect() as connection:
            # A transaction is always rolled back, but rollback is not a
            # substitute for database-enforced read-only access.
            with connection.begin() as transaction:
                result = connection.execution_options(yield_per=min(limit + 1, 100)).execute(
                    statement, inputs["parameters"]
                )
                if not result.returns_rows:
                    raise ReadError("sql.query_did_not_return_rows")
                columns = list(result.keys())
                if len(columns) > 128:
                    raise ReadError("sql.column_limit")
                rows, truncated, size = [], False, 0
                for row in result:
                    if len(rows) == limit:
                        truncated = True
                        break
                    normalized = [cell(value) for value in row]
                    size += len(encode(normalized).encode())
                    if size > maximum_bytes:
                        raise ReadError("sql.result_byte_limit")
                    rows.append(normalized)
                result.close()
                transaction.rollback()
        return {
            **common, "status": "executed", "executed": True,
            "query_id": inputs["query_id"], "columns": columns, "rows": rows,
            "returned_rows": len(rows), "max_rows": limit, "truncated": truncated,
            "result_description": query["result_description"],
            "observed_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "duration_ms": round((time.monotonic() - started) * 1000),
        }
    except ReadError:
        raise
    except Exception:
        # Drivers routinely embed SQL, bound values and URLs in exceptions.
        raise ReadError("sql.database_read_failed") from None
    finally:
        if engine is not None:
            engine.dispose()


def main():
    try:
        raw = sys.stdin.buffer.read(65537)
        if len(raw) > 65536:
            raise ReadError("sql.input_byte_limit")
        config = load_config(os.environ.get("RUNX_SQL_CONNECTION"))
        output = encode(read(json.loads(raw), config))
        if len(output.encode()) > config.get("max_bytes", MAX_BYTES):
            raise ReadError("sql.result_byte_limit")
        print(output)
    except ReadError as error:
        print(str(error), file=sys.stderr)
        return 2
    except Exception:
        print("sql.invalid_input_or_runtime", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
