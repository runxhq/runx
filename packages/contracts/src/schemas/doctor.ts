import { generatedSchema, generatedDefinition, validateContractSchema } from "../internal.js";
import type {
  DoctorDiagnosticContract,
  DoctorLocationContract,
  DoctorRepairContract,
  DoctorReportContract,
  DoctorSummaryContract,
} from "../generated/contract-types.js";

export const doctorV1Schema = generatedSchema<DoctorReportContract>("doctor.schema.json");
export const doctorDiagnosticSchema = generatedDefinition<DoctorDiagnosticContract>("DoctorDiagnostic");
export const doctorLocationSchema = generatedDefinition<DoctorLocationContract>("DoctorLocation");
export const doctorRepairSchema = generatedDefinition<DoctorRepairContract>("DoctorRepair");
export const doctorSummarySchema = generatedDefinition<DoctorSummaryContract>("DoctorSummary");

export function validateDoctorReportContract(value: unknown, label = "doctor_report"): DoctorReportContract {
  return validateContractSchema(doctorV1Schema, value, label);
}
