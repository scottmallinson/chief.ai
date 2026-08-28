# Changelog

## 0.2.0 (2026-08-28)

### Features

- **agent:** support Gemma tool retrieval ([e494f1f](https://github.com/scottmallinson/chief.ai/commit/e494f1fa061a7f5a54c54d1bec4b17dd3196873a))
- **ui:** list every connected account on the settings screen ([009ebb7](https://github.com/scottmallinson/chief.ai/commit/009ebb72419ed01675944d04e994b51d9e4b3cc1))
- **ui:** drive sign-in per account rather than per service ([21db814](https://github.com/scottmallinson/chief.ai/commit/21db81463fc4adc1db2020b4b1cbe0d1e78dd233))
- **db:** return the account a work log entry came from ([9b62297](https://github.com/scottmallinson/chief.ai/commit/9b622970685891a4fd08bef776238535ecbe3a8d))
- **agent:** answer about every connected GitHub account ([c991c14](https://github.com/scottmallinson/chief.ai/commit/c991c14c88df7e79fb8679e06de77c6486e13158))
- **daemon:** log work per account rather than per service ([94d25da](https://github.com/scottmallinson/chief.ai/commit/94d25da295cc2e9166ccd6c85cafe35c4d686aa6))
- **integrations:** take the service by name rather than per command ([6ba0dd5](https://github.com/scottmallinson/chief.ai/commit/6ba0dd5894380903c736bb96119fee877f2db162))
- **auth:** renew a credential once per account, not per service ([b323643](https://github.com/scottmallinson/chief.ai/commit/b323643fad61b2aad9be6255cd337aa4a97e447c))
- **auth:** describe a provider once, for the shared machinery ([9c904dd](https://github.com/scottmallinson/chief.ai/commit/9c904dd5009c3cae4a7a63955ca80d120be9c878))
- **db:** store one credential per connected account ([a9593b4](https://github.com/scottmallinson/chief.ai/commit/a9593b42d4b900348a221f600d9c831afe034423))
- **db:** hold many labelled accounts per service ([c3962bd](https://github.com/scottmallinson/chief.ai/commit/c3962bd40083229a26e658b23997b561500232f0))
- **auth:** add the loopback redirect listener ([b035d25](https://github.com/scottmallinson/chief.ai/commit/b035d255cc195967fb47889329debed617c47fee))
- **auth:** add PKCE verifier and state nonce ([0c6dd1c](https://github.com/scottmallinson/chief.ai/commit/0c6dd1cc531cf270dd736318ac1cdbc9f2c763d1))

### Fixes

- **repo:** follow the renamed package in the release version bump ([cfe91d0](https://github.com/scottmallinson/chief.ai/commit/cfe91d0b8b203fef8c516b93304114e8918258c8))
- **tauri:** quote the engine instead of guessing why it stopped ([d8bcf24](https://github.com/scottmallinson/chief.ai/commit/d8bcf24a0f0dc403767962a7752dee69dd54337c))
- **agent:** merge adjacent turns for Gemma templates ([20836b4](https://github.com/scottmallinson/chief.ai/commit/20836b4c4bf51d07f85b8893166f2c1477c821e9))
- **ui:** stop a long account name crowding out its own field ([d730df4](https://github.com/scottmallinson/chief.ai/commit/d730df4b02259597980582b4fb874953176734ad))
- **repo:** find toolchains in Git hooks ([6359876](https://github.com/scottmallinson/chief.ai/commit/63598763aa1a49e853f15f149ba97ad4107a5a2a))
- **ui:** put a refused account name back ([7dde80b](https://github.com/scottmallinson/chief.ai/commit/7dde80b79c513d71d336e46a877b53662ae053fb))
- **ui:** let an abandoned sign-in be given up on ([a3cc5cd](https://github.com/scottmallinson/chief.ai/commit/a3cc5cda435127f8966ebd4e2d79784c91926277))
- **ui:** stop a rename swallowing the click that ended it ([718a7ae](https://github.com/scottmallinson/chief.ai/commit/718a7ae412d283ef4cc3360bbb5c4e45f16429e3))
- **ui:** declare the fields a work log entry actually carries ([32c206d](https://github.com/scottmallinson/chief.ai/commit/32c206d1504ac9ccca13d4d09e349b63448f8bce))
- **integrations:** say what happened when a service is unknown ([7459049](https://github.com/scottmallinson/chief.ai/commit/7459049574aba984ee4ac30d223a56f40c6f0d0a))
- **daemon:** let one account's failure be its own ([47b74f4](https://github.com/scottmallinson/chief.ai/commit/47b74f404b1d10ecf7b9743a34d192c47392bb6c))
- **integrations:** adopt the account an upgrade left behind ([deea4ed](https://github.com/scottmallinson/chief.ai/commit/deea4edb913f537eb50f3d2e0627f31db7b983b2))
- **ui:** call the account-first integration commands ([a531d7e](https://github.com/scottmallinson/chief.ai/commit/a531d7e27b5c115160a7ca53053436e1239a339f))
- **db:** keep unattributed work log entries inside the dedupe index ([23e778c](https://github.com/scottmallinson/chief.ai/commit/23e778c61e1202ee4863bc79e6cf8c315195b4c9))
- **auth:** say what the sign-in redirect actually reported ([23741ae](https://github.com/scottmallinson/chief.ai/commit/23741ae60dc6b9660fe93193707a917eb2980f44))
- **auth:** answer every redirect connection at once ([dc734d8](https://github.com/scottmallinson/chief.ai/commit/dc734d8e1846a3374bd2cc131777062d80f08fa3))
- **auth:** make oauth module public to unblock clippy ([cf9a9ce](https://github.com/scottmallinson/chief.ai/commit/cf9a9ceda541cf80234a332391899cf2d6691036))
