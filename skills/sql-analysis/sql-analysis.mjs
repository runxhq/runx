// Relevance and evidence binding belong to the skill. The connector owns SQL
// and drivers; native data.validate owns full parameter-schema validation.
export function bindChoice({ question, source_ref, catalog, choice, max_rows }) {
  if (catalog.source_ref !== source_ref || catalog.status !== "catalog" || catalog.executed !== false) {
    throw new Error("The query catalog does not belong to this source or is not a catalog read.");
  }
  const query = catalog.queries.find((item) => item.id === choice.query_id);
  const ready = choice.decision === "ready" && Boolean(query);
  return {
    candidate: {
      decision: ready ? "ready" : "no_suitable_query",
      question, source_ref, dialect: catalog.dialect,
      query_id: ready ? query.id : "",
      parameters: ready ? choice.parameters : {},
      max_rows: Math.min(max_rows, catalog.max_rows),
      reason: ready ? choice.reason : choice.decision === "ready"
        ? "The selected query is absent from the declared source catalog." : choice.reason,
      missing_context: choice.missing_context,
      result_description: ready ? query.result_description : "",
      executed: false,
    },
    parameters_schema: ready ? query.parameters_schema : { type: "object", additionalProperties: false },
    parameters: ready ? choice.parameters : {},
  };
}

export function finalizePlan({ candidate, validation }) {
  return { sql_analysis_plan: {
    ...candidate,
    decision: candidate.decision === "ready" && !validation.valid ? "invalid_parameters" : candidate.decision,
    validation,
  }};
}

export function stopped({ plan }) {
  return { analysis_result: {
    decision: plan.decision, executed: false,
    answer: plan.reason, missing_context: plan.missing_context, plan,
  }};
}

export function finish({ question, plan, provider_operation, interpretation }) {
  const result = provider_operation.result;
  if (result.source_ref !== plan.source_ref || result.query_id !== plan.query_id
      || result.status !== "executed" || result.executed !== true
      || result.returned_rows !== result.rows.length || result.returned_rows > plan.max_rows) {
    throw new Error("Returned SQL evidence does not match the planned source, query and row ceiling.");
  }
  return { analysis_result: {
    decision: "answered", executed: true, question,
    answer: interpretation.answer, observations: interpretation.observations,
    caveats: [...interpretation.caveats, ...(result.truncated
      ? ["The query result is truncated at the admitted row ceiling; it is not a complete population."] : [])],
    plan, provider_operation,
  }};
}
