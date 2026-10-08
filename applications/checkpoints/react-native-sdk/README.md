# React Native SDK design history

The reusable SDK and app migration are implemented. These records preserve the
audited baseline, decisions, original exit criteria and bounded cutover evidence.
They do not form a second backlog or a current setup guide.

- [September 22 investigation](2026-09-22-investigation.md): why the app bridge was
  not a general SDK and which ownership boundaries needed to move.
- [September 22 implementation plan](2026-09-22-implementation-plan.md): the ordered
  migration, lifetime rules, removal targets and original acceptance criteria.
- [Implementation evidence](implementation-evidence.md): initial source, binding,
  package and mobile results, including failed checks and evidence limits.

Use the [current SDK guide](../../docs/react-native-sdk-implementation.md) for
ownership and remaining qualification, the [workspace guide](../../README.md)
for setup, and the [roadmap](../../docs/roadmap.md) for current product work.
Source links inside the historical audits identify the audited modules; the
files at today's checkout may differ from the recorded revision.
