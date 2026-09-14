# Changelog

## 0.5.0 (2026-09-14)

### Features

- **ui:** publish the changelog as a page on the site ([e1c4107](https://github.com/scottmallinson/chief.ai/commit/e1c4107e4b9cb7d46cc0521d6dd8720247fb4041))
- **repo:** add the marketing website ([f6ca6b4](https://github.com/scottmallinson/chief.ai/commit/f6ca6b4c774a682ff3d109d079ef3741ee919fd8))
- **integrations:** read Jira and Confluence with a pasted API token ([ca3e5c0](https://github.com/scottmallinson/chief.ai/commit/ca3e5c086e9286e6b6bec8f626a2b28885739180))
- **integrations:** read Jira through Atlassian's MCP server ([6822535](https://github.com/scottmallinson/chief.ai/commit/68225357c7f8689c632b779fb8ae5f62826abd6b))
- **daemon:** ingest the work the log has to answer questions about ([a0c07ec](https://github.com/scottmallinson/chief.ai/commit/a0c07ec33ab0476f46ed6d39cbe6a3c688d208b2))
- **daemon:** make Refresh read the accounts, and say what it found ([94c1db9](https://github.com/scottmallinson/chief.ai/commit/94c1db93c5d86bfa2bbdcb2186dba34d72e049c8))

### Fixes

- **ui:** align the site header on one baseline, and fit it on a phone ([0f35e21](https://github.com/scottmallinson/chief.ai/commit/0f35e21015e9bbc1b982cda51975beaf6cfd59a8))
- **ui:** restore the source links, and make the header button legible ([c8e782b](https://github.com/scottmallinson/chief.ai/commit/c8e782b6fffb36cbdb37fb92f1dd9bf06d8a5fa0))
- **deps:** take the patched fast-uri and js-yaml ([a43593b](https://github.com/scottmallinson/chief.ai/commit/a43593b617c9525889480235f1d95fbb54c37381))
- **integrations:** name a certificate failure rather than blaming the network ([801e40a](https://github.com/scottmallinson/chief.ai/commit/801e40aa10b8dfd96c3aa60dcb38aa05bc59cf59))
- **integrations:** say why Atlassian could not be reached, and name Chief ([2fd5c33](https://github.com/scottmallinson/chief.ai/commit/2fd5c3363f441be6374027c3828b2a378cb8460d))
- **ui:** line a connected account's button up with its field ([598a469](https://github.com/scottmallinson/chief.ai/commit/598a469e47b32463541cdab98cf1f53ad35d56f9))
- **auth:** say what happened when a second account is added ([cea5058](https://github.com/scottmallinson/chief.ai/commit/cea505884436c549f60ff42e911da7605f88306b))
- **agent:** stream the brief, so silence means silence ([011c407](https://github.com/scottmallinson/chief.ai/commit/011c407dd96aee7ec5ad15a3a70206cb266f4359))
- **agent:** stop the brief being handed a list of dates to echo ([61d40ea](https://github.com/scottmallinson/chief.ai/commit/61d40ea7e6bc21fcd6515f89a15ebc3086144677))
- **ui:** keep a work-log row inside the card it is in ([2949617](https://github.com/scottmallinson/chief.ai/commit/2949617d73ec410918a1e6887c602d6f99775019))
- **agent:** say what is missing rather than letting the model invent it ([d9c4222](https://github.com/scottmallinson/chief.ai/commit/d9c4222140020aea7cfcf45ee0bf56ceecf21802))
- **auth:** open GitHub with the code already in it ([f327c5c](https://github.com/scottmallinson/chief.ai/commit/f327c5cf938c44862baba87a6180e5509f4f2bba))
- **auth:** let this machine say which OAuth registration to sign in with ([3ee5f67](https://github.com/scottmallinson/chief.ai/commit/3ee5f678ecca2891ce8e028b46bdf9ade419126b))
- **agent:** answer with what happened, not with the word for its state ([dfbe45d](https://github.com/scottmallinson/chief.ai/commit/dfbe45d090ce0cc64979a70fc854b9b44b87a26c))
- **agent:** route the questions Chief itself suggests ([ca657f6](https://github.com/scottmallinson/chief.ai/commit/ca657f6c9c93c2160ebcbffc8664fef98738bf0f))
- **daemon:** stop the background pass failing silently ([0505335](https://github.com/scottmallinson/chief.ai/commit/050533572ca0c9737614212089102231cfd3854a))
- **ui:** keep today reachable, so a brief can always be asked for ([dc126b8](https://github.com/scottmallinson/chief.ai/commit/dc126b8dde5bfed283c97726f6ecd283c0e4203b))

## 0.4.0 (2026-09-03)

### Features

- **agent:** give the prompt path a seam, without a capability flag ([31cd9d3](https://github.com/scottmallinson/chief.ai/commit/31cd9d33d76cf9fdf960957e6cef22a57c5bf45a))
- **agent:** cap the chat prompt, and measure the figure the target meant ([7d8bbf3](https://github.com/scottmallinson/chief.ai/commit/7d8bbf335e0e67f7a690dc84bdace29a8b64868d))
- **agent:** say where an answer came from, in words Rust wrote ([798589a](https://github.com/scottmallinson/chief.ai/commit/798589ac8d681c33c9f91a2e92798d230ca465e2))
- **corpus:** roll old work into a monthly journal, deleting nothing ([87a1510](https://github.com/scottmallinson/chief.ai/commit/87a1510f4e28a89f004649ba658e28684fdb540b))
- **ui:** let the feed open the thing an entry is about ([cd58712](https://github.com/scottmallinson/chief.ai/commit/cd58712005903d46d26d5febb7e5346cc40f4d2c))
- **tauri:** restrict the local data directories to their owner ([6be8c1c](https://github.com/scottmallinson/chief.ai/commit/6be8c1c32fc984a3888d6d0b1ed393ce8da7c695))
- **ui:** say how fresh each connection is, and which one needs you ([8b997b6](https://github.com/scottmallinson/chief.ai/commit/8b997b603f8d7f5ebe2013f79e98c47121b80833))
- **daemon:** take the pass interval from settings, and read two clocks ([b6632ab](https://github.com/scottmallinson/chief.ai/commit/b6632ab722da4197c8b747217bdfb34de34cddb4))
- **daemon:** ingest deterministically, and record what each account is doing ([c812eb6](https://github.com/scottmallinson/chief.ai/commit/c812eb6e6d415040c0a6add8438cac3c8058123c))
- **agent:** answer read questions from the work log, not the network ([a2ead2e](https://github.com/scottmallinson/chief.ai/commit/a2ead2eb11c08b234c89b3a02af81f1158f31c8f))
- **db:** structure the work log and index it for search ([2a1ccd8](https://github.com/scottmallinson/chief.ai/commit/2a1ccd8e24f31999cb9537055cde6d37e7313dec))
- **agent:** answer the question the composer has been offering all along ([4dc0d6d](https://github.com/scottmallinson/chief.ai/commit/4dc0d6d77354d757af3fc405f257fe98d7c38c7f))
- **integrations:** read what Linear says is assigned to you ([69146dc](https://github.com/scottmallinson/chief.ai/commit/69146dccc249ba2b6b488c3f806c76d43f15e360))
- **corpus:** read a calendar the user subscribed to, with nothing to register ([3889248](https://github.com/scottmallinson/chief.ai/commit/388924844e72aa77ac1369ae37240f22682f7dab))
- **db:** notice the file the user edited in their own editor ([58d5a21](https://github.com/scottmallinson/chief.ai/commit/58d5a216bbc73ede2b0b2c7efa00530e5f14e9b5))
- **agent:** draft the thing before the user asks, and send none of it ([f8aed24](https://github.com/scottmallinson/chief.ai/commit/f8aed249e9e0e6de433a2db08920884ddecc4c27))
- **agent:** seed the profile from the user's own work, with their consent ([1687dfc](https://github.com/scottmallinson/chief.ai/commit/1687dfcd0b848aaa108a07d6d37c2ee366dfef97))
- **agent:** answer the questions this machine already knows, without a model ([e0d238e](https://github.com/scottmallinson/chief.ai/commit/e0d238e99266391563332592764fafe61ea78965))
- **ui:** put the brief on screen, and move chat into a drawer over it ([deaf9fb](https://github.com/scottmallinson/chief.ai/commit/deaf9fb875cb05e5fc1ce1513ef096f37b5333a5))

### Fixes

- **integrations:** make disconnecting an account delete its data ([8083e93](https://github.com/scottmallinson/chief.ai/commit/8083e93f42e78f299a0c7b22f7b5e5e964ebd34b))
- **ui:** stop a regex lookbehind opening the app as a white screen (#59) ([fb50b0d](https://github.com/scottmallinson/chief.ai/commit/fb50b0de02e1ae12a293c65d8b811006d9c484fa))
- **agent:** stop a long answer being killed, and thrown away, at five minutes ([93ee3b0](https://github.com/scottmallinson/chief.ai/commit/93ee3b0d1305d2d549f77bda58df1756224a5f25))

## 0.3.1 (2026-08-29)

### Fixes

- **ci:** build the macOS Intel bundle against an Intel engine ([2b59c65](https://github.com/scottmallinson/chief.ai/commit/2b59c6595bc9c5bc1a1ab3f15a1c3579a99c0720))

## 0.3.0 (2026-08-29)

### Features

- **agent:** write the day's brief without asking a model to plan it ([0c6447e](https://github.com/scottmallinson/chief.ai/commit/0c6447e52c36b5ef4440d0b134f5fd28ce182411))
- **db:** keep the corpus in a folder the user can open ([26cd9a7](https://github.com/scottmallinson/chief.ai/commit/26cd9a7b2bc5a3951bb8b2e1ac1ec097a163ae9d))
- **agent:** answer from the calendar and the inbox ([a61944a](https://github.com/scottmallinson/chief.ai/commit/a61944a0c4c96cb84520fa032c69550c8c82e2a3))
- **auth:** sign in to Outlook in the browser ([c06e8df](https://github.com/scottmallinson/chief.ai/commit/c06e8dfe06024bcb0dc519dd281f446678fd2335))
- **tauri:** let this machine say how fast it actually answers ([accb8c8](https://github.com/scottmallinson/chief.ai/commit/accb8c8ba491dd4c1e7db5020b5158b8c7b0c7be))
- **tauri:** give the model's memory back when nothing is using it ([036cb8f](https://github.com/scottmallinson/chief.ai/commit/036cb8f076d0be422108ef63bed0d9538fba4c92))
- **tauri:** download a model the machine can hold, and say which ([a39ce1d](https://github.com/scottmallinson/chief.ai/commit/a39ce1dd2c153b87fce065d534624f2cabdb1931))
- **tauri:** size the engine to the machine it is running on ([8635fce](https://github.com/scottmallinson/chief.ai/commit/8635fce52eb82bb6bcf5925358230bf92420718a))

### Fixes

- **agent:** leave a brief alone once somebody has edited it ([2202bee](https://github.com/scottmallinson/chief.ai/commit/2202bee12d4b79095bac8546939cd2a151361070))
- **agent:** tell the retry what it could not look up ([9d36c9b](https://github.com/scottmallinson/chief.ai/commit/9d36c9b66b78a480e40955babdd3994c78c14254))
- **agent:** shorten a tool result by dropping entries, not by cutting the text ([49f7868](https://github.com/scottmallinson/chief.ai/commit/49f7868c2fc16f0b5f7279f0f6eab981efb83a9b))
- **agent:** stop the token estimate under-counting what it most has to hold back ([dcd502b](https://github.com/scottmallinson/chief.ai/commit/dcd502bfaf64c6e4bb01d0db12b53824ce86c667))
- **agent:** charge tool results against the prompt budget, and say when an answer was cut ([5e1e67c](https://github.com/scottmallinson/chief.ai/commit/5e1e67c3685f591952d2cc98ee7fff91f9c1b8c0))
- **tauri:** say when the engine on our port is not ours to stop ([90c2157](https://github.com/scottmallinson/chief.ai/commit/90c2157bd274e93d613612fc9c2d1db5d17a6c42))
- **agent:** never send the model a prompt with no question in it ([04d225d](https://github.com/scottmallinson/chief.ai/commit/04d225df1704d8b6b6519ceba8c5957309cab5ea))
- **daemon:** keep the engine alive for as long as a pass takes ([86b273e](https://github.com/scottmallinson/chief.ai/commit/86b273eaf3f18c8ad2c132033689206cfb2524d9))
- **agent:** answer the question even when no tool fits it ([1f15c55](https://github.com/scottmallinson/chief.ai/commit/1f15c55d66ab2ab2a11c9dbb2999dd3416827c45))
- **agent:** answer with the model's own tool calling again ([1131db1](https://github.com/scottmallinson/chief.ai/commit/1131db16b730e686bb5da14c8534a77419012fd7))

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
