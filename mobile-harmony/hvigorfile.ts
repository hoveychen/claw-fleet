import { appTasks, OhosAppContext, OhosPluginId } from '@ohos/hvigor-ohos-plugin';
import { hvigor } from '@ohos/hvigor';

// Inject release signing from environment variables — does not use local materials in build-profile.json5.
//
// Why we cannot directly commit signing materials to build-profile.json5:
//   1. Public repos must not contain keys; the signingConfigs in the repo is an empty array, and the local
//      version is protected by `git update-index --skip-worktree` (so DevEco on this machine can still sign normally).
//   2. DevEco encrypts keyPassword/storePassword with its own platform-specific encryption — the string
//      encrypted on macOS may not decrypt on Linux CI. So CI can only use plaintext passwords + environment variables.
//
// Required environment variables (if any are missing, the entire process is skipped and build-profile.json5 remains unchanged):
//   FLEET_OHOS_STORE_FILE       .p12 keystore
//   FLEET_OHOS_CERT_PATH        .cer release certificate
//   FLEET_OHOS_PROFILE_PATH     .p7b release Profile
//   FLEET_OHOS_KEY_ALIAS        key alias
//   FLEET_OHOS_STORE_PASSWORD   keystore password (plaintext)
//   FLEET_OHOS_KEY_PASSWORD     key password (plaintext, defaults to STORE_PASSWORD)
//   FLEET_OHOS_SIGN_ALG         defaults to SHA256withECDSA
const REQUIRED = [
  'FLEET_OHOS_STORE_FILE',
  'FLEET_OHOS_CERT_PATH',
  'FLEET_OHOS_PROFILE_PATH',
  'FLEET_OHOS_KEY_ALIAS',
  'FLEET_OHOS_STORE_PASSWORD',
];

const SIGNING_CONFIG_NAME = 'fleet-env';

hvigor.getRootNode().afterNodeEvaluate(node => {
  const missing = REQUIRED.filter(k => !process.env[k]);
  if (missing.length === REQUIRED.length) {
    // Not planning to use env signing (local DevEco development uses this path by default), silently skip.
    return;
  }
  if (missing.length > 0) {
    // Partial materials are more dangerous than none: hvigor produces an *unsigned* package instead of erroring
    // when signing config is missing, so "I clearly configured signing" won't surface until install/upload fails.
    throw new Error(
      `[fleet] 发布签名环境变量不完整,缺: ${missing.join(', ')}。` +
      `要么把它们补全,要么全部不设(改回 build-profile.json5 的本地签名)。`
    );
  }

  const ctx = node.getContext(OhosPluginId.OHOS_APP_PLUGIN) as OhosAppContext;
  const profileOpt = ctx.getBuildProfileOpt();

  profileOpt['app']['signingConfigs'] = [
    {
      name: SIGNING_CONFIG_NAME,
      type: 'HarmonyOS',
      material: {
        storeFile: process.env.FLEET_OHOS_STORE_FILE,
        certpath: process.env.FLEET_OHOS_CERT_PATH,
        profile: process.env.FLEET_OHOS_PROFILE_PATH,
        keyAlias: process.env.FLEET_OHOS_KEY_ALIAS,
        storePassword: process.env.FLEET_OHOS_STORE_PASSWORD,
        keyPassword: process.env.FLEET_OHOS_KEY_PASSWORD ?? process.env.FLEET_OHOS_STORE_PASSWORD,
        signAlg: process.env.FLEET_OHOS_SIGN_ALG ?? 'SHA256withECDSA',
      },
    },
  ];

  // Setting signingConfigs alone is insufficient — each product must explicitly reference it. DevEco itself
  // misses this step (tested 2026-08-18): materials present but reference missing, hvigor reports
  // "No signingConfig found for product default", SignHap idles 2ms, produces only
  // entry-default-unsigned.hap. So we add the reference here as well.
  for (const product of profileOpt['app']['products'] ?? []) {
    product['signingConfig'] = SIGNING_CONFIG_NAME;
  }

  ctx.setBuildProfileOpt(profileOpt);
  console.log(`[fleet] 已用环境变量覆盖签名配置 (${SIGNING_CONFIG_NAME})`);
});

export default {
  system: appTasks, /* Built-in plugin of Hvigor. It cannot be modified. */
  plugins: []       /* Custom plugin to extend the functionality of Hvigor. */
}
