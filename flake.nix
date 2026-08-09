{
  description = "fujin (zellijプラグイン) の開発環境";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };
        # nixpkgs の rustc は wasm32-wasip1 の std を持たないので rust-overlay を使う。
        # 「latest」は flake.lock で固定されるため、上がるのは nix flake update のときだけ
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ "wasm32-wasip1" ];
        };
      in {
        devShells.default = pkgs.mkShell {
          # mkShell では開発ツールは packages に置く
          # （buildInputs は「ビルド対象がリンクするライブラリ」の意味になる）
          packages = [
            rustToolchain
            pkgs.git-cliff
            # zellij 本体。zellij は client/server のバージョンが揃わないと
            # attach も zellij action も弾くので、この版は Cargo.toml の
            # zellij-tile と合わせておく（ずれると常駐セッションを操作できない）
            pkgs.zellij
          ];
        };
      });
}
