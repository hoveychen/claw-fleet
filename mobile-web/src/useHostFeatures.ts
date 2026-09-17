// Which optional panes desktop host has on startup. Currently one: terminal (backend
// FLEET_TERMINAL, see claw-fleet-core/src/feature_flags.rs). Mobile can't derive it
// itself, asks relay (mobile_relay.rs::serve_request `host_features`).
//
// **Default all off**: no answer (relay not connected, request in flight, old desktop
// doesn't know method) always treat as off. Opposite (assume on, reject, retract) lets
// user open terminal page, shell opens, hits "terminal feature is disabled" — entry
// existence shouldn't be told by a failed request. Desktop equivalent is store.ts
// loadHostFeatures, also fail-closed.
import { useEffect, useState } from "react";
import type { HostFeatures } from "./generated/types";
import type { FleetTransport } from "./transport";

const ALL_OFF: HostFeatures = { terminal: false };

/** Normalize one response to a trusted switch set. Separate function because every
 *  value here is "prefer off": old desktop may answer null or omit fields, relay's JSON
 *  may send true as string "true" — truthiness check treats string as on, `=== true`
 *  doesn't. */
export function normalizeHostFeatures(raw: unknown): HostFeatures {
  const terminal = (raw as HostFeatures | null | undefined)?.terminal;
  return { terminal: terminal === true };
}

export function useHostFeatures(client: FleetTransport | null): HostFeatures {
  const [features, setFeatures] = useState<HostFeatures>(ALL_OFF);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    client
      .request<HostFeatures>("host_features")
      .then((r) => {
        if (alive) setFeatures(normalizeHostFeatures(r));
      })
      .catch(() => {
        if (alive) setFeatures(ALL_OFF);
      });
    return () => {
      alive = false;
    };
  }, [client]);
  return features;
}
