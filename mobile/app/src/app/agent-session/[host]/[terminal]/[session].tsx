// One of the background sessions a terminal's agent view holds, as a chat.
// Its own route rather than the terminal's chat with a query: the terminal's
// route is the list it was opened from, under it in the stack.

export { Chat as default } from '../../../session/[host]/[terminal]';
