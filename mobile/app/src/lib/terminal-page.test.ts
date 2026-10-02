import { describe, expect, jest, test } from '@jest/globals';
import { page } from './terminal-page';

jest.mock('./xterm-assets', () => ({ XTERM_CSS: '/* xterm */', XTERM_JS: '// xterm', XTERM_WEBGL_JS: '// webgl' }));

describe('terminal page', () => {
  test('embeds assets and the native bridge contract', () => {
    expect(page).toContain('<style>/* xterm */');
    expect(page).toContain('<script>// xterm</script>');
    expect(page).toContain('<script>// webgl</script>');
    expect(page).toContain('window.ReactNativeWebView.postMessage');
    expect(page).toContain('window.ket = function (message)');
    expect(page).toContain("message.type === 'reset'");
    expect(page).toContain("message.type === 'write'");
    expect(page).toContain("message.type === 'interactive'");
  });
});
