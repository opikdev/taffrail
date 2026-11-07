# 002. Default to read-only with no telemetry

- **Status**: accepted
- **Context**: The app handles cluster credentials, so an unknown binary must earn trust.
- **Decision**: Ship read-only with no accounts, telemetry or paid tier, and require no contributor agreement that allows relicensing.
- **Consequences**:
  - Users can try it safely on production clusters.
  - Writes such as apply, scale and delete need a later decision, with a per-context write mode.
  - The project gets no usage data, so demand signals must come from issues.
