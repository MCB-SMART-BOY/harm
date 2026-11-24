# ADR 0004: prediction expiry and provenance

Date: 2026-11-24. Status: accepted.

Every future prediction must carry observation time, horizon, expiry, uncertainty, model/device identity, and inference cost. State has exactly one provenance class: observed, predicted, assumed, or oracle. Oracle information is offline-only and must not enter a production controller. This invariant is planned but not yet enforced by a prediction/control API.
