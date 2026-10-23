# SQL

Run parameterized database reads through a local SQLAlchemy dialect and driver.
The same connector can bind SQLite, PostgreSQL, MySQL, SQL Server, Oracle or an
installed third-party dialect. There is no database-engine list in Runx core.
An engine needs both an installed SQLAlchemy dialect/DBAPI driver and a SQLGlot
parser dialect for its declared statements; unsupported configurations fail.

The operator declares useful queries once. Agents discover their descriptions,
choose a query and supply typed values. Neither an agent nor a skill supplies
executable SQL, a URL, a driver, or connection options. This is suitable for
repeatable production analytics; interactive arbitrary SQL belongs in a
separately authorized database session.

## Local setup

Use Python 3.10 or later. Install `tools/sql/read/requirements.txt` in the Python
environment whose `python3` is on the Runx host's PATH. Install the chosen DBAPI
driver and, for external engines, its SQLAlchemy dialect in that environment.
PostgreSQL with `postgresql+psycopg` uses `psycopg`; SQLite uses Python's driver.
Dependencies are optional connector dependencies, never CLI dependencies.

Bind one `connection` credential to the local connector instance. Its value is
the JSON configuration below. Runx delivers it only to this trusted provider
process as `RUNX_SQL_CONNECTION`; never put it in skill inputs or public files.
The process runs with host privileges under native process supervision. It is
not the isolated JavaScript skill worker.

```json
{
  "source": "analytics:production",
  "url": "postgresql+psycopg://report_reader:PASSWORD@localhost/analytics",
  "dialect": "postgres",
  "read_only": true,
  "connect_args": {
    "connect_timeout": 5,
    "options": "-c statement_timeout=10000 -c default_transaction_read_only=on"
  },
  "max_rows": 100,
  "queries": {
    "qualified_trials_by_campaign": {
      "description": "Rank campaigns by qualified trials on or after a supplied date.",
      "sql": "SELECT source_campaign_id, COUNT(*) AS qualified_trials FROM trials WHERE qualified_at >= :since GROUP BY source_campaign_id ORDER BY qualified_trials DESC LIMIT 100",
      "parameters_schema": {
        "type": "object",
        "additionalProperties": false,
        "required": ["since"],
        "properties": {"since": {"type": "string", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}$"}}
      },
      "result_description": "Campaign identifier and qualified-trial count. A null campaign is unattributed; the query does not establish causal attribution."
    }
  }
}
```

Use a database account that cannot write, execute mutating functions, or read
other data. For SQLite, open an existing file read-only, for example
`sqlite:///file:/absolute/path/analytics.db?mode=ro&uri=true` with dialect
`sqlite`. For other engines configure equivalent database permissions and a
server statement timeout through the account, `connect_args` or
`execution_options`. These options pass to SQLAlchemy unchanged.

`read_only: true` is the operator's declaration of that binding, not proof of
database permissions. The connector parses one query and rejects DML, DDL,
mutating CTEs, SELECT INTO and locking reads. A SELECT can call functions with
side effects: the database account is the access boundary. Query definitions
and installed dialect plugins must therefore come from the operator, never an
untrusted model response. Every call closes its connection and rolls back its
transaction; rollback does not replace permissions or undo external effects.

## Operations and results

`catalog.read` takes `{action: "catalog", source}`. It returns descriptions,
parameter schemas and result semantics without opening a connection. It does
not publish SQL or connection settings. `query.read` takes
`{action: "query", source, query_id, parameters, max_rows}` and executes the
declared statement using SQLAlchemy bound parameters. Parameter contracts use
JSON Schema 2020-12 with no external retrieval. SQL uses named `:parameter`
bindings, and every binding must have a declared, required property. Missing,
extra or invalid values fail before a connection is opened. Validation uses the
same Rust JSON Schema engine as native `data.validate`, through its Python
binding; formats are annotations and external retrieval is disabled.

Results contain ordered `columns` and row arrays, the source and query identity,
observation time, duration and an explicit `truncated` flag. Do not treat a
truncated result as a complete population or turn correlation into attribution.
Numbers outside JavaScript's exact integer range, decimals, date/time values
and binary values use tagged cells; they are never silently rounded. Unhandled
driver-specific types fail rather than being coerced into misleading strings.

The source can lower the hard ceilings of 1,000 rows and 2 MiB; callers can lower
the row ceiling further. At most 128 columns are returned. Results stream where
the driver supports streaming; the connector fetches at most the row limit plus
one. This bounds returned evidence, not the database's intermediate computation
or a driver's internal buffering. Bound expensive queries in their definitions
and configure server timeouts. Native supervision terminates the provider
process after 30 seconds. A killed process is not proof a remote server stopped
work immediately.

On failure the process returns a stable error code. It never returns driver
exceptions, URLs, SQL or bound values in error messages. Fix source configuration,
permissions, parameter values or query cost at their owner; retrying with broader
authority is not an automatic recovery.

Local integration checks run separately from core CI:

```sh
python3 -m unittest discover -s connectors/sql/tests -p '*_test.py'
```

`RUNX_SQL_TEST_POSTGRES_URL` optionally supplies a disposable PostgreSQL test
database. Its owner creates a temporary read-only role and schema and removes
them after the check. Do not point it at production.

Driver and dialect details: [SQLAlchemy dialects](https://docs.sqlalchemy.org/en/20/dialects/),
[SQLGlot](https://github.com/tobymao/sqlglot).
