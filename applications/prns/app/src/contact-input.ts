import { verifyMessagingContact } from "./contacts";
import type { SavedContact } from "./contacts";

// UI ingress budget, deliberately larger than the codec's bounded signed URI.
// The canonical decoder/verifier remains the authority on framing and signature.
export const contactInputBudget = 4096;
export function reviewContactInput(input: string, alias = ""): SavedContact {
  if (input.length > contactInputBudget) throw new Error("This code is too large to be a contact card. Ask for a prns contact QR code or link.");
  return verifyMessagingContact(input, alias);
}
