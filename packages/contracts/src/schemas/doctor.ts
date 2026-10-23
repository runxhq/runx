import { generatedSchema, generatedSchemaAt, validateContractSchema } from "../internal.js";
import type {
  DoctorDiagnosticContract,
  DoctorLocationContract,
  DoctorRepairContract,
  DoctorReportContract,
  DoctorSummaryContract,
} from "../generated/contract-types.js";

export const doctorV1Schema = generatedSchema<DoctorReportContract>("doctor.schema.json");
export const doctorDiagnosticSchema = generatedSchemaAt<DoctorDiagnosticContract>(
  doctorV1Schema,
  ["properties", "diagnostics", "items"],
  "doctor.diagnostics[]",
);
export const doctorLocationSchema = generatedSchemaAt<DoctorLocationContract>(
  doctorDiagnosticSchema,
  ["properties", "location"],
  "doctor.diagnostics[].location",
);
export const doctorRepairSchema = generatedSchemaAt<DoctorRepairContract>(
  doctorDiagnosticSchema,
  ["properties", "repairs", "items"],
  "doctor.diagnostics[].repairs[]",
);
export const doctorSummarySchema = generatedSchemaAt<DoctorSummaryContract>(
  doctorV1Schema,
  ["properties", "summary"],
  "doctor.summary",
);

export function validateDoctorReportContract(value: unknown, label = "doctor_report"): DoctorReportContract {
  return validateContractSchema(doctorV1Schema, value, label);
}
