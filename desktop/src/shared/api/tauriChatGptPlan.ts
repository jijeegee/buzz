import { invokeTauri } from "@/shared/api/tauri";

/** Kuzz's own "Sign in with ChatGPT" login. Never carries a token. */
export type ChatGptPlanStatus = {
  signedIn: boolean;
  /** False when the user declined ChatGPT plan use on the consent screen. */
  planEnabled: boolean;
  email: string | null;
};

export type ChatGptPlanSignIn = {
  status: ChatGptPlanStatus;
  /** First sign-in on this computer: show the one-time plan notice. */
  firstSignIn: boolean;
};

export const CHATGPT_MANAGE_USAGE_URL = "https://chatgpt.com/settings/usage";

export function getChatGptPlanStatus(): Promise<ChatGptPlanStatus> {
  return invokeTauri<ChatGptPlanStatus>("chatgpt_plan_status");
}

export function signInChatGptPlan(): Promise<ChatGptPlanSignIn> {
  return invokeTauri<ChatGptPlanSignIn>("chatgpt_plan_sign_in");
}

/** Resolves to whether OpenAI confirmed the session revocation. */
export function signOutChatGptPlan(): Promise<boolean> {
  return invokeTauri<boolean>("chatgpt_plan_sign_out");
}
