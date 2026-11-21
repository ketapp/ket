// Words from the desktop, made ready to show a person.

/** A reason from the desktop as a sentence: its first letter a capital, and
 * a full stop unless it already ends as a sentence does — an ellipsis
 * included, so a reason still under way doesn't gain a stop after it. */
export function sentence(text: string): string {
  const s = text.charAt(0).toUpperCase() + text.slice(1);
  return /[.!?…]$/.test(s) ? s : `${s}.`;
}

/** `text` with its first letter a capital and nothing added: for a line that
 * is not a sentence, such as what an agent is doing. */
export function capitalised(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** What went wrong, in words: an error's message, or whatever was thrown. */
export function reasonOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
