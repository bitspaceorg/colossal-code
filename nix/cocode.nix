{ ... }:
{
    perSystem =
        { pkgs, ... }:
        let
            python = pkgs.python3.withPackages (ps: [ ps.python-frontmatter ]);
            cocode-package = pkgs.rustPlatform.buildRustPackage {
                pname = "cocode";
                version = "0.1.0";
                src = ./..;

                cargoLock = {
                    lockFile = ../Cargo.lock;
                    allowBuiltinFetchGit = true;
                };

                nativeBuildInputs = with pkgs; [
                    pkg-config
                    python3
                ];
                buildInputs = with pkgs; [ openssl ];
                doCheck = false;

                meta = {
                    description = "Terminal-first coding assistant";
                    license = pkgs.lib.licenses.mit;
                    platforms = pkgs.lib.platforms.unix;
                    mainProgram = "cocode";
                };
            };
        in
        {
            _module.args.cocode-deps = {
                nativeBuildInputs = with pkgs; [
                    pkg-config
                    rustc
                    cargo
                    clippy
                    sccache
                    python
                ];
                buildInputs = with pkgs; [ openssl ];
            };

            packages.default = cocode-package;

            apps.default = {
                type = "app";
                program = "${cocode-package}/bin/cocode";
            };
        };
}
