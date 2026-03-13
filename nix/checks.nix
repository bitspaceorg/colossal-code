{ ... }:
{
    perSystem =
        { self', ... }:
        {
            checks.unit-test = self'.packages.default.overrideAttrs (_oldAttrs: {
                name = "cocode-unit-test";
                doCheck = true;
                checkPhase = ''
                    cargo test --all-targets --no-run
                '';
            });
        };
}
