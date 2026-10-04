# Glossary

`dev artifacts`: code, test, schema, or other artifacts that are part of the development process.
`requirement`: a statement of expected behavior or required structure of the system, from high-level boundary behavior down to exact implementation-level facts.
`requirements graph`: the canonical graph of requirement nodes and edges that decompose, order, condition, trace, and synchronize requirements with `dev artifacts`.
`boundary requirement`: a high-level requirement describing externally meaningful behavior or an entrypoint-facing capability.
`leaf requirement`: a lowest-level requirement that maps to exact code-backed behavior or structure, such as a branch outcome, constant, assignment, return value, field mapping, error, side effect, callable, parameter, or schema field.
`decomposition requirement`: a requirement that explains part of a higher-level requirement and may itself reference lower-level requirements.
`at rest`: when no changes to requirements or dev artifacts are in-flight and the bidirectional sync process has been run.
`domain update`: the incremental process of gathering and storing domain knowledge based on analysis of the `dev artifacts`.
`boundary`: a callable (or requirement about it) that defines a business driven boundary of a system.
`entrypoint`: a callable (or requirement about it) that has no incoming calls in the local call graph.
`callable`: a function, method, or other code unit that can be executed.