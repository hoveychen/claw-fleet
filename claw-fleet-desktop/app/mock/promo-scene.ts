export const PROMO_SCENES = ["base", "guard", "ask"] as const;

export type PromoScene = (typeof PROMO_SCENES)[number];

export function promoSceneFromSearch(search: string): PromoScene | null {
  const value = new URLSearchParams(search).get("promo");
  return PROMO_SCENES.includes(value as PromoScene) ? (value as PromoScene) : null;
}

export function primePromoStorage(storage: Storage): void {
  storage.setItem("mock-store:onboarding-dismissed", "true");
  storage.setItem(
    "mock-store:onboarding-seen-features",
    JSON.stringify([
      "appearance",
      "notifications",
      "hooks_guard_elicitation",
      "global_ask",
      "prd_discipline",
      "wiki_guidance",
      "model_guidance",
      "skill_interop",
    ]),
  );
}
