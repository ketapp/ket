// A picked file's bytes in a browser, where the pickers hand back a `blob:`
// or `data:` address: see read-file.ts.

export async function readFile(uri: string): Promise<Uint8Array> {
  const response = await fetch(uri);
  return new Uint8Array(await response.arrayBuffer());
}
