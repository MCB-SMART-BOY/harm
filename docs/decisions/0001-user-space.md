# ADR 0001: user-space runtime

Date: 2026-11-24. Status: accepted.

Use a user-space runtime and no kernel patch in the first study. This keeps development reversible and permits unprivileged telemetry; kernel scheduler modifications would confound the research question and deployment cost. Any future actuator is constrained to explicitly granted cgroup scope.
