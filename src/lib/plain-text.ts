/**
 * Chat text with Markdown marks taken out.
 *
 * The chat draws answers as plain text, so a model's `**bold**` or `## Heading`
 * shows as stray asterisks and hashes. The system prompt asks for plain text and
 * a small model forgets; this is the part that does not depend on it remembering.
 * It runs at display time, so it also covers an answer while it is still arriving.
 *
 * Deliberately narrow: it removes the marks small models actually write and
 * leaves every word alone. A lone `*` that is not wrapped round text, such as
 * `5 * 3`, is not touched.
 */
export function plainText(text: string): string {
  return text
    .split('\n')
    .map((line) =>
      line
        .replace(/^\s{0,3}#{1,6}\s+/, '')
        .replace(/^(\s*)[*•]\s+/, '$1- ')
        .replace(/(\*\*|__)(.+?)\1/g, '$2')
        .replace(/(^|[\s(])\*([^*\s][^*]*?)\*(?=$|[\s).,;:!?])/g, '$1$2')
        .replace(/`([^`]+)`/g, '$1'),
    )
    .join('\n')
    .replace(/\*\*|__/g, '');
}
