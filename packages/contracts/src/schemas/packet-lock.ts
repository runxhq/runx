import { generatedSchema, generatedSchemaAt } from "../internal.js";
import type { PacketLockContract, PacketLockEntryContract } from "../generated/contract-types.js";

export const packetLockV1Schema = generatedSchema<PacketLockContract>(
  "packet-lock.schema.json",
);
export const packetLockEntrySchema = generatedSchemaAt<PacketLockEntryContract>(
  packetLockV1Schema,
  ["properties", "packets", "items"],
  "packet-lock.packets[]",
);
