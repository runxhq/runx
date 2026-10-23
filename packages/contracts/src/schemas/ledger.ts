import { generatedSchema, generatedSchemaAt, validateContractSchema } from "../internal.js";
import type {
  LedgerChainContract,
  LedgerEntryContract,
  LedgerPayloadContract,
} from "../generated/contract-types.js";

export const ledgerEntrySchemaVersion = "runx.ledger.entry.v1" as const;
export const ledgerChainSchemaVersion = "runx.ledger.chain.v1" as const;
export const ledgerDigestAlgorithm = "sha256" as const;
export const ledgerCanonicalization = "runx.stable-json.v1" as const;

export const ledgerEntrySchema = generatedSchema<LedgerEntryContract>("ledger-entry.schema.json");
export const ledgerChainSchema = generatedSchemaAt<LedgerChainContract>(
  ledgerEntrySchema,
  ["properties", "chain"],
  "ledger_entry.chain",
);
export const ledgerPayloadSchema = generatedSchemaAt<LedgerPayloadContract>(
  ledgerEntrySchema,
  ["properties", "entry"],
  "ledger_entry.entry",
);

export function validateLedgerEntryContract(value: unknown, label = "ledger_entry"): LedgerEntryContract {
  return validateContractSchema(ledgerEntrySchema, value, label);
}

export function validateLedgerPayloadContract(value: unknown, label = "ledger_payload"): LedgerPayloadContract {
  return validateContractSchema(ledgerPayloadSchema, value, label);
}
