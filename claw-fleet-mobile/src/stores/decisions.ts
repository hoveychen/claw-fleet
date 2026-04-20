import { create } from "zustand";
import type {
  GuardRequest,
  GuardDecision,
  ElicitationRequest,
  ElicitationDecision,
  PendingDecision,
} from "../types";

interface DecisionsState {
  decisions: PendingDecision[];
  pendingCount: number;

  /** The decision ID to scroll to when panel opens (set by notification tap). */
  focusedId: string | null;

  addGuard: (request: GuardRequest) => void;
  addElicitation: (request: ElicitationRequest) => void;
  remove: (id: string) => void;
  setFocusedId: (id: string | null) => void;

  /** Update guard analysis result */
  setGuardAnalysis: (id: string, analysis: string | null) => void;
  setGuardAnalyzing: (id: string, analyzing: boolean) => void;

  /** Update elicitation step/selection state */
  setElicitationStep: (id: string, step: number) => void;
  toggleSelection: (id: string, question: string, label: string, multiSelect: boolean) => void;
  setCustomAnswer: (id: string, question: string, text: string) => void;
  setMultiSelectOverride: (id: string, question: string, override: boolean) => void;
}

export const useDecisionsStore = create<DecisionsState>((set) => ({
  decisions: [],
  pendingCount: 0,
  focusedId: null,

  setFocusedId: (id) => set({ focusedId: id }),

  addGuard: (request) =>
    set((state) => {
      if (state.decisions.some((d) => d.id === request.id)) return state;
      const decision: GuardDecision = {
        kind: "guard",
        id: request.id,
        request,
        analysis: null,
        analyzing: false,
        arrivedAt: Date.now(),
      };
      const decisions = [...state.decisions, decision];
      return { decisions, pendingCount: decisions.length };
    }),

  addElicitation: (request) =>
    set((state) => {
      if (state.decisions.some((d) => d.id === request.id)) return state;
      const decision: ElicitationDecision = {
        kind: "elicitation",
        id: request.id,
        request,
        step: 0,
        selections: {},
        customAnswers: {},
        multiSelectOverrides: {},
        arrivedAt: Date.now(),
      };
      const decisions = [...state.decisions, decision];
      return { decisions, pendingCount: decisions.length };
    }),

  remove: (id) =>
    set((state) => {
      const decisions = state.decisions.filter((d) => d.id !== id);
      return { decisions, pendingCount: decisions.length };
    }),

  setGuardAnalysis: (id, analysis) =>
    set((state) => ({
      decisions: state.decisions.map((d) =>
        d.id === id && d.kind === "guard"
          ? { ...d, analysis, analyzing: false }
          : d,
      ),
    })),

  setGuardAnalyzing: (id, analyzing) =>
    set((state) => ({
      decisions: state.decisions.map((d) =>
        d.id === id && d.kind === "guard" ? { ...d, analyzing } : d,
      ),
    })),

  setElicitationStep: (id, step) =>
    set((state) => ({
      decisions: state.decisions.map((d) =>
        d.id === id && d.kind === "elicitation" ? { ...d, step } : d,
      ),
    })),

  toggleSelection: (id, question, label, multiSelect) =>
    set((state) => ({
      decisions: state.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        const current = d.selections[question] ?? [];
        let updated: string[];
        if (multiSelect) {
          updated = current.includes(label)
            ? current.filter((l) => l !== label)
            : [...current, label];
        } else {
          updated = current.includes(label) ? [] : [label];
        }
        return {
          ...d,
          selections: { ...d.selections, [question]: updated },
        };
      }),
    })),

  setCustomAnswer: (id, question, text) =>
    set((state) => ({
      decisions: state.decisions.map((d) =>
        d.id === id && d.kind === "elicitation"
          ? { ...d, customAnswers: { ...d.customAnswers, [question]: text } }
          : d,
      ),
    })),

  setMultiSelectOverride: (id, question, override) =>
    set((state) => ({
      decisions: state.decisions.map((d) => {
        if (d.id !== id || d.kind !== "elicitation") return d;
        const nextOverrides = { ...d.multiSelectOverrides, [question]: override };
        let nextSelections = d.selections;
        if (!override) {
          const current = d.selections[question] ?? [];
          if (current.length > 1) {
            nextSelections = { ...d.selections, [question]: [current[0]] };
          }
        }
        return { ...d, multiSelectOverrides: nextOverrides, selections: nextSelections };
      }),
    })),
}));
