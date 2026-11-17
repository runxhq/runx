use super::CommandSpec;

pub const ROOT_COMMAND_SPEC: CommandSpec = CommandSpec {
    name: "cli.help",
    top_level_usage: &[],
    usage: &["runx <command> [args]", "runx --help", "runx --version"],
    notes: &[],
    options: &[
        "-h, --help",
        "-V, --version",
        "-j, --json  Emit the native command catalog with --help",
    ],
};

pub const COMMAND_SPECS: &[CommandSpec] = &[
    CommandSpec {
        name: "verify",
        top_level_usage: &[],
        usage: &[
            "runx verify [receipt-id] [--receipt-dir dir] [--receipt <path|->] [--trust home|configured] [--notary <path|-> --notary-key trusted.pem] [-j|--json]",
        ],
        notes: &[
            "A receipt id names one receipt: the report shows what it says, and verifies the tree it belongs to.",
            "Receipts verify against this home's local key and any key RUNX_RECEIPT_VERIFY_ISSUER_TYPE, RUNX_RECEIPT_VERIFY_KID and RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64 configure.",
        ],
        options: &[
            "--receipt-dir dir",
            "--receipt <path|->",
            "--trust home|configured  Trust this home's key and configured keys, or configured keys only",
            "--notary <path|->",
            "--notary-key trusted.pem",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "history",
        top_level_usage: &[],
        usage: &[
            "runx history [query] [--skill s] [--state s] [--since iso] [--until iso] [--limit n] [--json]",
            "runx history <run-id> [--after cursor] [--json]",
        ],
        notes: &[
            "Lists the workspace's runs, most recent first: running, paused (with the request each waits on and its resume command) or sealed (with its receipt).",
            "A run id of this workspace reads that run: its state, every request it waits on, whether a process drives it, and its events after --after, the cursor a previous read returned.",
        ],
        options: &[
            "--skill s",
            "--state s          running, paused, sealed, or a sealed outcome",
            "--since iso        Runs started at or after this instant",
            "--until iso        Runs started at or before this instant",
            "--limit n",
            "--after cursor     Read a run's events after this cursor",
            "--receipt-dir dir  Named in the resume commands it prints",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "process",
        top_level_usage: &[],
        usage: &[
            "runx process list [--actor ref] [-j|--json]",
            "runx process read <process-id> [--after cursor] [--max-bytes n] [-j|--json]",
            "runx process stop <process-id> [-j|--json]",
        ],
        notes: &[
            "The long-running processes the project's runs started with process.start, each under runx's supervision until it exits; a process outlives the run that started it.",
            "read returns its output after --after, the cursor a previous read returned; stop terminates it, then kills it.",
        ],
        options: &[
            "--actor ref     Only the processes this actor's runs started",
            "--after cursor  Read the output after this cursor (default 0)",
            "--max-bytes n   About how much output to read (default 65536)",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "resume",
        top_level_usage: &[],
        usage: &[
            "runx resume <run-id> [answers.json|-] [--receipt-dir dir] [--package-digest sha256] [--closure-digest sha256] [--drive [--drive-rounds n]] [--diagnostics] [-j|--json]",
        ],
        notes: &[
            "At a terminal with no answers file, it shows each request the run waits on, whole, and takes the answer: approve or decline with your reason, an option by its number, or your own words.",
            crate::resume::ANSWER_DOCUMENT_EXAMPLE,
            crate::resume::ANSWER_DOCUMENT_GUIDANCE,
            crate::resume::APPROVAL_DOCUMENT_GUIDANCE,
        ],
        options: &[
            "--receipt-dir dir",
            "--drive  Explicitly allow an in-process model loop for this continuation",
            "--drive-rounds n  Bound each driven turn to n rounds (default agent.rounds, else 4)",
            "--package-digest sha256   Reassert the checkpointed package binding",
            "--closure-digest sha256",
            "                          Reassert the checkpointed closure",
            "--diagnostics  Include execution trace and intermediate context",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "list",
        top_level_usage: &[],
        usage: &["runx list [tools|skills|packets] [--only valid|invalid] [-j|--json]"],
        notes: &[],
        options: &["--only valid|invalid", "-j, --json"],
    },
    CommandSpec {
        name: "login",
        top_level_usage: &[],
        usage: &[
            "runx login [--provider name] [--for default|publish] [--token-stdin] [--api-base-url url] [--allow-local-api] [-j|--json]",
        ],
        notes: &[],
        options: &[
            "--provider provider",
            "--for purpose",
            "--token-stdin        Sign in with a token from --provider, piped on stdin, instead of the browser",
            "--api-base-url url",
            "--allow-local-api",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "config",
        top_level_usage: &[
            "runx config set|get <provider|dialect|model|secret|endpoint-url> [value] | list | unset endpoint-url|dialect|secret [-j|--json]",
        ],
        usage: &[
            "runx config set <key> <value> [-j|--json]",
            "runx config get <key> [-j|--json]",
            "runx config list [-j|--json]",
            "runx config unset endpoint-url|dialect|secret [-j|--json]",
        ],
        notes: &[
            "Short keys: provider, dialect, model, secret, endpoint-url and auth-mode. Fully qualified config keys are also accepted, among them how a driven turn holds up: agent.retries, agent.output_tokens, agent.max_output_tokens, agent.request_timeout_seconds and agent.rounds. Config holds settings, never a secret.",
            "`secret` names the secret holding the model key, kept with `runx secret set <name>`. RUNX_AGENT_API_KEY or the provider's own variable (ANTHROPIC_API_KEY, OPENAI_API_KEY, GEMINI_API_KEY) also supplies it; two places holding different keys are refused.",
            "A runx account credential is stored by runx login, never set here; RUNX_PUBLIC_API_TOKEN supplies one for a single process.",
            "anthropic, openai and gemini are built-in providers. Any other provider also sets dialect (chat_completions, anthropic_messages or openai_responses) and endpoint-url.",
        ],
        options: &["-j, --json"],
    },
    CommandSpec {
        name: "authority",
        top_level_usage: &["runx authority save|list|show|remove ... [-j|--json]"],
        usage: &[
            "runx authority save <name> <file|-> [--workspace <path>...] [--standing] [-j|--json]",
            "runx authority list [-j|--json]",
            "runx authority show <name> [-j|--json]",
            "runx authority remove <name> [-j|--json]",
        ],
        notes: &[
            "A saved authority is the run authority a person gives ahead of time, kept by name in the runx home for the workspaces it lists, by default the one it's saved in. `runx skill`, `runx skill inspect` and `runx mcp` apply one where they name it with `--authority <name>`, and only in a workspace it lists, because a repository's .mcp.json can name one. One saved with `--standing` is what every run in its workspaces that states no authority starts with: it gives only the person's own data, delegations on private such as their memory, and a workspace has one. Nothing else applies one unnamed, and an explicit empty authority stays empty.",
            "It speaks in connector and package names: each delegation names its `connector` and holds one or more `terms` of `verbs` over `bounds`, and the run resolves each connector to the instance that stands for it when it starts. `save` checks it resolves in each workspace it lists before saving it. A runx home that fell back to the workspace, with HOME and USERPROFILE unset, keeps none.",
        ],
        options: &[
            "--workspace path  A workspace it's for; repeat for each",
            "--standing        What its workspaces' runs that state no authority start with",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "secret",
        top_level_usage: &["runx secret set|list|rm|read ... [-j|--json]"],
        usage: &[
            "runx secret set <name> [-j|--json]",
            "runx secret set <name> --from <reference> [-j|--json]",
            "runx secret list [-j|--json]",
            "runx secret rm <name> [-j|--json]",
            "runx secret read <name>",
        ],
        notes: &[
            "The one place a secret is kept: by name in the runx home, sealed there, or read through a secrets manager by reference. A connector binds one with `runx connector use <connector> --secret <name>`, the drive names one with `runx config set agent.secret <name>`, and a program reads one with `runx secret read <name>`. Setting a secret again serves everything that names it from its next call.",
            "`set` asks for the value without echo at a terminal, or reads it from stdin when piped; never from an argument. `--from <reference>` keeps it in a secrets manager instead: op://<vault>/<item>[/<section>]/<field>, aws-sm://<name or ARN>[#field], aws-ps://<parameter>[#field], vault://<mount>/<path>[#field], keychain://<service>/<account> or file:///<path>[#field]. runx reads it once to prove access and pin where it lives (a 1Password account, an ARN, a Vault address), and through the manager's own CLI and your sign-in on every use; the home keeps the reference, never the value.",
            "`list` shows each secret's name and where its value is, never the value. `rm` removes one; whatever still names it refuses until it is set again. `read` prints the value, as `gh auth token` and `op read` print theirs, for a program you run.",
            "A variable set in the environment or the workspace .env needs no secret and no setup: a connector whose credential it holds runs as it with no instance selected, reading it on every call, and the drive reads its provider's variable.",
        ],
        options: &["--from reference", "-j, --json"],
    },
    CommandSpec {
        name: "loops",
        top_level_usage: &["runx loops [-j|--json]"],
        usage: &["runx loops [-j|--json]"],
        notes: &[
            "Every loop in this workspace: the method it calls, whether it runs, the run working its inbox, how its last run ended, its next wake, and what waits on you: a notice and its action, a change it proposes, a run of yours that's open. A loop is a file in loops/, a named, continuing job that calls one method of a skill each time something wakes it, and keeps everything it knows in its own record.",
        ],
        options: &["-j, --json"],
    },
    CommandSpec {
        name: "loop",
        top_level_usage: &["runx loop new|show|accept|decline|retry|skip|message ..."],
        usage: &[
            "runx loop new <skill> <name>",
            "runx loop show <loop> [-j|--json]",
            "runx loop accept <loop> <proposal> [--authority <name>]",
            "runx loop decline <loop> <proposal> [--why <words>]",
            "runx loop retry <loop>",
            "runx loop skip <loop>",
            "runx loop message <loop> <text>",
        ],
        notes: &[
            "`new` copies the skill's loop template to loops/<name>.yaml for you to fill in. `show` says everything the loop's record holds of it now: its notices, and each proposal with its id.",
            "A run never changes its loop: it proposes. `accept` takes a change to the loop's brief or inputs, writing the file's next revision, or makes a new loop's file under the authority you name, not started. `decline` keeps your words.",
            "After a run that changed something and then failed, or one admission refused, the loop pauses for you. `retry` gives its batch to a new attempt; `skip` moves past it. `message` tells the loop something, which wakes it.",
        ],
        options: &["--authority <name>", "--why <words>", "-j, --json"],
    },
    CommandSpec {
        name: "start",
        top_level_usage: &["runx start|stop <loop>"],
        usage: &["runx start <loop>", "runx stop <loop> [--why <words>]"],
        notes: &[
            "`start` runs a loop from now on, on its file as it is: it can't start until its inputs are what its method takes. It also resumes a paused loop and takes a revision that widens what the loop may do (another method, authority or policy), which waits for a start. `stop` lets a run that's working finish, and then no wake follows.",
        ],
        options: &["--why <words>"],
    },
    CommandSpec {
        name: "run",
        top_level_usage: &["runx run <loop> [method] [--inputs <json>]"],
        usage: &[
            "runx run <loop>",
            "runx run <loop> <method> [--inputs <json>]",
        ],
        notes: &[
            "With no method, the loop wakes once now, whether it runs or not. With one, it's your own run of that method on the loop: in the loop's records, under its authority, with the loop's inputs the method takes and yours over them. It never takes the loop's events, so a review you leave open never stops the loop. One is open per method: running it again opens the open one, and `runx resume` answers what it asks.",
        ],
        options: &["--inputs <json>"],
    },
    CommandSpec {
        name: "on",
        top_level_usage: &["runx on|off|status"],
        usage: &["runx on", "runx off", "runx status"],
        notes: &[
            "`on` starts the always-on runx in the background: one per runx home, running every started loop's wakes as they come due, and nothing else while it sleeps. A sleeping or switched-off machine runs nothing; when it's back, what came due runs once, marked late. `off` lets the wake it's in reach its end, then stops. `status` says whether it's on.",
        ],
        options: &[],
    },
    CommandSpec {
        name: "workspace",
        top_level_usage: &["runx workspace list|rebind|forget ... [-j|--json]"],
        usage: &[
            "runx workspace list [-j|--json]",
            "runx workspace rebind <old path> [-j|--json]",
            "runx workspace forget <old path> [-j|--json]",
        ],
        notes: &[
            "Each workspace you run runx in is named by an id the runx home mints the first time a step reads $run.workspace_id there, keyed by where the workspace is: its path as its git repository's main checkout holds it, so every worktree of one clone is one workspace, and outside git its own path. A workspace is a directory with .git, pnpm-workspace.yaml, skills/ or connectors/; elsewhere a run reads a null workspace id. The id is never read from the workspace, so a repository can't name another workspace's memory, and a home that fell back to the workspace mints none.",
            "runx can't tell a move from a copy, so a moved workspace is minted a new id. `rebind <old path>`, run in the workspace where it is now, gives it back the id it had at the old path; the id minted for it here goes, and what was kept under that id stays under it. `list` shows each id and its path.",
            "`forget <old path>` removes a workspace whose folder is gone for good, with what was kept under its id: every stream of the person's private store that names it, its memory among them, its id in each saved authority, removing one that was for it alone, and the id itself, so no workspace is offered it as a move again. It's the person's statement, so it asks nothing, and a run can't make it. A folder still there is refused.",
        ],
        options: &["-j, --json"],
    },
    CommandSpec {
        name: "trust",
        top_level_usage: &["runx trust <reference>[#method] [-j|--json]"],
        usage: &["runx trust <reference>[#method] [-j|--json]"],
        notes: &[
            "You state your trust in a package ahead of any turn that loads it. It binds the package as a load would, with the method you name or each method it declares, and records each closure in your runx home as your approval of a load's trust would, so a later load of it binds at once. A changed package is another closure, and asks again.",
            "Running it is the statement, so it asks nothing. It prints what it recorded: each closure, its method, the connectors it binds and the package's tier. It's refused inside a runx run, so an agent working in one can't state your trust, and no MCP tool offers it.",
        ],
        options: &["-j, --json"],
    },
    CommandSpec {
        name: "connector",
        top_level_usage: &["runx connector list|inspect|use|revoke|import-mcp ... [-j|--json]"],
        usage: &[
            "runx connector list [-j|--json]",
            "runx connector inspect <connector> [-j|--json]",
            "runx connector use <connector> [<instance|grant>] [--scope <scope>...] [--parameter <name>=<value>...] [-j|--json]",
            "runx connector use <connector> --session [--scope <scope>...] [-j|--json]",
            "runx connector use <connector> --credential <name> [--scope <scope>...] [--parameter <name>=<value>...] [-j|--json]",
            "runx connector use <connector> [--credential <name>] --secret <name> [--scope <scope>...] [--parameter <name>=<value>...] [-j|--json]",
            "runx connector use <connector> --hosted [--secret <name>] [--scope <scope>...] [--target <locator>] [-j|--json]",
            "runx connector revoke <instance|grant> [-j|--json]",
            "runx connector import-mcp <url> [--secret <name>] [-j|--json]",
            "runx connector import-mcp [-j|--json] -- <command> [args...]",
        ],
        notes: &[
            "A connector whose credential's variable is set, in the environment or the workspace .env, needs no setup: with no instance selected a run binds it to that variable as `use` would, storing nothing; two of its credentials set at once refuse. `use` is the one place a connector's authority is otherwise set up: an instance or grant confirmed earlier, the signed-in local session, a credential's variable, a secret `runx secret` keeps, or a new runx Connect grant. With nothing named it takes the one available path, asks for the credential of a connector that takes only one, and refuses with the exact forms when there are several. It never picks between accounts for you; an instance bound to a variable or a secret acts as whichever account it holds. A connector's sign-in command is never run to see what is available; binding it takes `--session`. A variable the environment and the workspace .env set to different values is refused.",
            "A credential whose variable (declared by its connector) is set in the environment or the workspace .env is bound to that variable: the instance records the name, never the value, and each call reads it. Otherwise the value is asked for without echo at a terminal, or read from stdin when piped, and kept as a secret named after the connector; a secret already set under that name is bound with `--secret`, never taken. `--secret <name>` binds a secret by name: the instance records the name and a proof only this runx home can make, never the value, and each call reads the secret again, so setting it anew serves without binding again. `--hosted` opens a Connect grant, sends the credential its hosted placement names when it takes one (from `--secret`, or read the same way), prints the authorization link, waits up to two minutes, and binds the grant; when it is not yet authorized it prints the command that finishes it. Connect keeps its own copy. With --json it returns at once with the link and that command. --scope narrows what the instance serves (a new grant requests every scope its operations declare otherwise); --target narrows a new grant to one resource (owner/repo, or a locator).",
            "`list` shows each connector in reach with the instance it uses, then every instance and the signed-in account's grants; `runx secret list` shows the secrets. `revoke` removes one instance or grant (and the instances standing for it). A run never falls back to another instance or account; instances live in the project's kernel store (RUNX_PROJECT_DIR, else .runx).",
            "A connector has one name, such as github or google/calendar. Outside the skill's plugin, a skill pins the connector's manifest_digest, and an operator installs it with `runx registry install <name>`.",
            "`import-mcp` lists the tools an MCP server serves with the digest a connector manifest pins each by; the bearer token, when the server needs one, is the secret `--secret` names. After `--`, it starts the stdio server that command line names and reads its tools.",
        ],
        options: &[
            "--session",
            "--credential name",
            "--secret name",
            "--hosted",
            "--scope scope",
            "--target locator",
            "--parameter name=value",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "notarize",
        top_level_usage: &[],
        usage: &[
            "runx notarize <receipt.json> [--api-base-url url] [--allow-local-api] [-j|--json]",
        ],
        notes: &[
            "The hosted notary counter-seals the receipt and publishes the notarized record: anyone can read it at its public URL.",
            "Check the counter-seal with runx verify --notary <document> --notary-key <key.pem>.",
        ],
        options: &[
            "--api-base-url url  Public API base URL (default: RUNX_PUBLIC_API_BASE_URL or https://api.runx.ai)",
            "--allow-local-api   Allow loopback/private public API URLs for local dogfood only",
            "-j, --json          Print the raw notary response as JSON",
        ],
    },
    CommandSpec {
        name: "doctor",
        top_level_usage: &[],
        usage: &["runx doctor [path|authority|registry] [--json]"],
        notes: &[],
        options: &["-j, --json"],
    },
    CommandSpec {
        name: "export",
        top_level_usage: &[],
        usage: &["runx export <claude|codex> [skill-ref...] [--project] [--json]"],
        notes: &[
            "A full export syncs the current skill workspace and prunes only shims it owns; named skill refs update additively.",
        ],
        options: &[
            "--project  Write project-local host-agent shims",
            "-j, --json",
        ],
    },
    CommandSpec {
        name: "mcp",
        top_level_usage: &[],
        usage: &[
            "runx mcp <skill-ref[#method]...> [--authority name] [--receipt-dir dir] [--http-listen [addr]] [--http-allow-non-loopback]",
            "runx mcp --run <run-id> [--receipt-dir dir] [--http-listen [addr]] [--http-allow-non-loopback]",
        ],
        notes: &[
            "The governed MCP server. Each skill is a tool: a ref with #method offers that method under the skill's name, and a ref without one offers its default method and each other method that makes calls. A skill call that pauses for agent work is worked on the same connection through turn_list, turn_status, turn_read, turn_call and turn_complete, naming its run, and call_cancel cancels one of its calls while it executes. The server works only the runs its own calls opened.",
            "--run serves one paused run's open turns, for a host that hands its agent that run alone. Each call is admitted against the turn's allowance and the run's authority, executed and recorded; a call that needs approval stays pending until runx resume answers it; turn_complete ends the turn and the run continues.",
        ],
        options: &[
            "--run <run-id>  Serve this run's open turns",
            "--authority name  Start every run a skill call opens under this saved authority (runx authority); with none, the workspace's standing one, if one is saved",
            "--receipt-dir dir",
            "--http-listen [addr]",
            "--http-allow-non-loopback",
        ],
    },
    CommandSpec {
        name: "skill",
        top_level_usage: &[],
        usage: &[
            "runx skill <skill-ref|publisher/name@version|skill-dir|SKILL.md>[#method] [-i name=value...] [--inputs file|-] [--act json] [--run-authority json | --authority name] [--max-spend usd] [--context json] [--run-id id] [-j] [--diagnostics] [--drive [--drive-rounds n]] [--registry url] [--digest sha256] [--package-digest sha256 --closure-digest sha256] [--receipt-dir dir]",
            "runx skill inspect <skill-ref|publisher/name@version|skill-dir|SKILL.md>[#method] [--inputs file|-] [--run-authority json | --authority name] [-j] [--registry url] [--digest sha256]",
        ],
        notes: &[
            "#method names the method to run or inspect. A ref with none runs the package's default_method, or its only method. A package directory or a file in one is that package, never a registry name.",
        ],
        options: &[
            "-i, --input name=value   Set an input, read by the type its method declares: a string as written, any other type as JSON; repeat for each input",
            "--inputs file|-          Read the complete JSON input object from a file or stdin; do not mix with -i",
            "--act json               State the act the run is invoked with, as a JSON act with its purpose",
            "--run-authority json     The authority a host states for this run or inspection, as a JSON run authority: its grants, delegations, packages, effects, reach and limits. It needs no workspace and saves nothing",
            "--authority name         Run under the preset an operator saved as name (runx authority), which applies only in a workspace it lists; with neither, the workspace's standing one, if one is saved",
            "--max-spend usd           Bound the root run's operation quotes and moved USD/USDC amounts. Calls without a USD quote, or over what remains, are refused before dispatch (cost_limit_exceeded); an absent quote never means free",
            "--context json           The caller's context for the run: {current_context, historical_context} entries; it informs the run, never grants authority",
            "--run-id id              The run's submission identity: submitting it again continues that run, and a sealed run returns what it sealed",
            "--drive          Explicitly allow an in-process model loop for this run",
            "--drive-rounds n Bound each driven turn to n rounds (default agent.rounds, else 4)",
            "--receipt-dir dir    Write receipts under dir",
            "-j, --json               Print machine-readable output",
            "--diagnostics            Show everything: the complete prepared context before the run, and its trace and intermediate context inline",
            "--registry url           The hosted registry a registry ref is fetched from",
            "--digest sha256          Refuse a fetched package whose registry content digest differs",
            "--package-digest sha256 Bind execution to the complete validated skill package; requires closure digest",
            "--closure-digest sha256",
            "                          Bind execution to the complete native skill closure; requires package digest",
        ],
    },
    CommandSpec {
        name: "test",
        top_level_usage: &[],
        usage: &[
            "runx test <package-dir|SKILL.md|tests/<name>.yaml...> [--receipt-dir dir] [-j|--json]",
        ],
        notes: &["A package runs each of its tests/*.yaml, in name order."],
        options: &["--receipt-dir dir", "-j, --json"],
    },
    CommandSpec {
        name: "tool",
        top_level_usage: &[],
        usage: &[
            "runx tool search <query> [--source source] [--json]",
            "runx tool inspect <ref> [--source source] [--json]",
        ],
        notes: &[],
        options: &["--source source", "-j, --json"],
    },
    CommandSpec {
        name: "registry",
        top_level_usage: &["runx registry search|install|package|publish ... [-j|--json]"],
        usage: &[
            "runx registry search <query> [--registry url] [--limit n] [-j|--json]",
            "runx registry install <publisher>/<name>[@version] [--registry url] [--digest sha256] [-j|--json]",
            "runx registry install --records file|- [--registry url] [-j|--json]",
            "runx registry package <SKILL.md|connector.yaml|package-dir> [-j|--json]",
            "runx registry publish <SKILL.md|connector.yaml|package-dir> [--registry url] [--publisher slug] [-j|--json]",
        ],
        notes: &[
            "Each verb takes a skill or a connector. `package` and `publish` read the kind from the path: connector.yaml, or a directory holding one, is a connector; SKILL.md or its directory is a skill.",
            "`install` adds what is published under the name to the project. A skill carries the connectors it binds, so a name published as both adds the skill; a connector alone adds the connector, for building your own skill on it. A name and version always name one package. A name alone takes the newest version this runx reads: when a newer one needs another runx, it is named beside what was installed. Running a skill with `runx skill <ref>` fetches it the same way, so most work needs no install.",
            "A registry is a hosted service, named by its URL: --registry, else RUNX_REGISTRY_URL, else the runx catalog. A package is its directory, its own run.yaml its profile. A publish records the version its run.yaml states, or one named by its content when it states none, and the registry grants its trust tier.",
            "Connectors are published to, searched in and installed from a hosted registry, each into the publisher its ref names.",
        ],
        options: &[
            "--registry url           The hosted registry, by its URL",
            "--digest sha256",
            "--records file|-         One JSON array of signed version records the registry served, from a file or stdin, each installed as it was signed",
            "--publisher slug         The namespace a package is published into",
            "--limit n",
            "-j, --json",
        ],
    },
];
