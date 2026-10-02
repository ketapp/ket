// The phone side of the ket remote protocol. See client.ts to connect,
// noise.ts for the handshake, and the generated contract in gen/.

export { type Greeting, type PairedHost, Remote, confirmationCode, envelope, pair, resume, MAJOR, MINOR } from './client.ts';
export { type Keypair, generateKeypair, keypairFrom } from './noise.ts';
export { type Offer, parseOffer, fromBase64Url, toBase64Url } from './offer.ts';
export { utf8 } from './bytes.ts';
export * from './gen/ket/remote/v1/remote_pb.js';
// The app builds messages with the same protobuf runtime the contract was
// generated for, rather than a second copy of it.
export { create, fromBinary, toBinary } from '@bufbuild/protobuf';
