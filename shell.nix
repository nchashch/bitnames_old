{ pkgs ? import <nixpkgs> {} }:

with pkgs;

mkShell {
  buildInputs = [
    protobuf
  ];
  PROTOC="${protobuf.out}/bin/protoc";
  PROTOC_INCLUDE="${protobuf.out}/include";
}
