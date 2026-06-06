//! fleet.yaml configuration carrier (REQ-047, design §6.2).
//!
//! Scaffold created by master before Wave 3 so P15 can fill this without
//! racing P10 on `lib.rs`. P15 implements: parse `phases: [{name, cmd,
//! resources}]` and `resources: { custom_X: { concurrency: N } }`, validate,
//! and feed custom locks to the scheduler. See design/tasks-impl-plan.md (P15).
