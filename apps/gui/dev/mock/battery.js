import { jitter } from "./common.js";

export function createBatteryFixture({ params, getTick, publishSource }) {
  const DESKTOP = params.get("pc") === "desktop";
  const BATTERY_MODE = params.get("battery") || (DESKTOP ? "none" : "discharging");

  const BATTERY_REF = "battery-00000000-0000-4000-8000-00000000b001";

  const BATTERY_REF_2 = "battery-00000000-0000-4000-8000-00000000b002";

  let batteryRead = null;

  function batteryView(ref, at, values) {
    return {
      battery_ref: ref, kind: "system", capacity_units: "milliwatt_hours", measured_at_unix_ms: at,
      charge_percent: null, charge_state: null, on_ac: null, critical: null, power_rate_mw: null,
      estimated_runtime_windows_seconds: null, estimated_runtime_at_current_rate_seconds: null,
      remaining_capacity_mwh: null, full_charge_capacity_mwh: null, design_capacity_mwh: null,
      capacity_retention_percent: null, estimated_wear_percent: null, cycle_count: null, inconsistent: false,
      ...values,
    };
  }

  function absoluteBattery(ref, at, { charge, design, full, ...values }) {
    const remaining = Math.round((charge / 100) * full);
    return batteryView(ref, at, {
      charge_percent: charge, remaining_capacity_mwh: remaining, full_charge_capacity_mwh: full, design_capacity_mwh: design,
      capacity_retention_percent: (full / design) * 100, estimated_wear_percent: 100 - (full / design) * 100,
      ...values,
    });
  }

  function dischargingBattery(ref, at) {
    const power = -Math.round(jitter(8400, 600));
    const battery = absoluteBattery(ref, at, { charge: 78.1 - 0.02 * getTick(), design: 57000, full: 52800, charge_state: "discharging", on_ac: false, critical: false, power_rate_mw: power, estimated_runtime_windows_seconds: 17400 - 5 * getTick(), cycle_count: 212 });
    battery.estimated_runtime_at_current_rate_seconds = (battery.remaining_capacity_mwh * 3600) / -power;
    return battery;
  }

  const BATTERY_FIXTURES = {
    discharging: (at) => [dischargingBattery(BATTERY_REF, at)],
    // The development PC's readings (docs/battery.md): no power, cycles or time left reported.
    ac: (at) => [absoluteBattery(BATTERY_REF, at, { charge: 100, design: 47310, full: 39900, charge_state: "idle", on_ac: true, critical: false, capacity_retention_percent: 84.3373, estimated_wear_percent: 15.6627 })],
    charging: (at) => [absoluteBattery(BATTERY_REF, at, { charge: 46, design: 57000, full: 52800, charge_state: "charging", on_ac: true, critical: false, power_rate_mw: Math.round(jitter(23500, 800)), cycle_count: 212 })],
    low: (at) => {
      const power = -Math.round(jitter(11200, 600));
      const battery = absoluteBattery(BATTERY_REF, at, { charge: 6, design: 57000, full: 52800, charge_state: "discharging", on_ac: false, critical: true, power_rate_mw: power, estimated_runtime_windows_seconds: 1260, cycle_count: 212 });
      battery.estimated_runtime_at_current_rate_seconds = (battery.remaining_capacity_mwh * 3600) / -power;
      return [battery];
    },
    // A percentage only: no energy, power, calculated time left or wear (docs/battery.md).
    relative: (at) => [batteryView(BATTERY_REF, at, { capacity_units: "relative", charge_percent: 64, charge_state: "discharging", on_ac: false, critical: false, estimated_runtime_windows_seconds: 9000 })],
    two: (at) => [
      dischargingBattery(BATTERY_REF, at),
      absoluteBattery(BATTERY_REF_2, at, { charge: 91.4, design: 24000, full: 23200, charge_state: "idle", on_ac: false, critical: false, power_rate_mw: 0, cycle_count: 87 }),
    ],
    unavailable: () => [],
    none: () => [],
  };

  function liveBatteries() {
    const fixture = BATTERY_FIXTURES[BATTERY_MODE] || BATTERY_FIXTURES.discharging;
    if (!batteryRead || (getTick() % 2 === 1 && !window.__pcagentMock?.holdBattery)) {
      const at = Date.now() - 40;
      const batteries = fixture(at);
      batteryRead = { batteries, sourceId: batteries.length ? publishSource([{ kind: "resource", resource: "battery" }], at, { battery: batteries }) : null };
    }
    return { batteries: batteryRead.batteries, battery_source_id: batteryRead.sourceId, battery_unavailable: BATTERY_MODE === "unavailable" };
  }
  return { mode: BATTERY_MODE, liveBatteries };
}
