/**
 * A command a person copies into a shell, carried as its arguments, mirrored
 * from `runx-contracts::copyable_command` and held to the same vectors
 * (`fixtures/contracts/copyable-command`). Every surface shows and copies
 * `copy_text`, rendered from `argv` here, never assembled from display strings.
 */
export interface CopyableCommand {
  readonly shell: "posix";
  readonly argv: readonly string[];
  readonly copy_text: string;
}

const BARE_WORD = /^[A-Za-z0-9@%+:,./_-]+$/u;
// eslint-disable-next-line no-control-regex
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/u;

/** The command `argv` for a POSIX shell; throws on no arguments or a control character. */
export function posixCommand(argv: readonly string[]): CopyableCommand {
  return { shell: "posix", argv: [...argv], copy_text: renderPosixCommand(argv) };
}

/**
 * `argv` as one POSIX shell line: an argument of only letters, digits and
 * `@%+:,./_-` stands bare; any other is single-quoted, with each `'` written
 * as `'\''`.
 */
export function renderPosixCommand(argv: readonly string[]): string {
  if (argv.length === 0) throw new Error("A command has at least one argument.");
  return argv
    .map((argument) => {
      if (CONTROL.test(argument)) throw new Error(`Command argument ${JSON.stringify(argument)} holds a control character.`);
      return BARE_WORD.test(argument) ? argument : `'${argument.replaceAll("'", "'\\''")}'`;
    })
    .join(" ");
}
