// ESM shim for `protobufjs/minimal`.
//
// pbjs-generated modules in this repo use:
//   import * as $protobuf from "protobufjs/minimal";
//
// In an ESM/SSR context, `protobufjs/minimal` is CommonJS and the namespace import
// does not expose `roots`, `Reader`, etc as named exports, which can crash SSR.
// This shim re-exports the relevant pieces as real ESM named exports.
import pb from 'protobufjs/minimal.js';

export const Reader = pb.Reader;
export const Writer = pb.Writer;
export const util = pb.util;
export const roots = pb.roots;

// Optional exports (harmless) in case generated code ever needs them.
export const rpc = pb.rpc;
export const configure = pb.configure;
export const build = pb.build;

export default pb;
