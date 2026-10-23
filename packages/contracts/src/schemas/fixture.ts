import { generatedSchema } from "../internal.js";
import type { FixtureContract } from "../generated/contract-types.js";

export const fixtureV1Schema = generatedSchema<FixtureContract>("fixture.schema.json");
