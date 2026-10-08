#!/bin/sh
if [ "${RUNX_INPUTS_JSON+x}" = "x" ]; then
  printf '%s' "$RUNX_INPUTS_JSON"
else
  printf '%s' '{}'
fi
