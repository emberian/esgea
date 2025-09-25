# esgea - Sneaky Guys!

an exhillerating two-player asymmetric-information game facilitated by computer!

there isn't a good way to play this game yet, although the mechanics should all be complete.

the server prevents some forms of cheating, but itself is an omniscient party. Work is(n't) underway to design a trustless server.

## WebAssembly demo

You can play a lightweight demonstration of the mechanics without running a server. The
`wasm-app` crate compiles the game logic to WebAssembly and drives the static interface in
`web/index.html`.

To build the demo locally:

```bash
wasm-pack build wasm-app --target web --out-dir dist/pkg
cp -r web/* dist/
python3 -m http.server --directory dist 9000
```

Open <http://localhost:9000> in a browser to try the demo. A GitHub Actions workflow deploys
the same build to GitHub Pages on every push to `main`.

- [ ] fix remaining TODO in source code
- [ ] WebTransport(?) stateless(?) server(?)
- [ ] web frontend with SVG(?) maps(?)
- [ ] editors/god mode
- [ ] cryptography to avoid cheating / privileged server
- [ ] computer player AI to play against
- [ ] match recording/replay
- [ ] scoring/lobby/matchmaking
