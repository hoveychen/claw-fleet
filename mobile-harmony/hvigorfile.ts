import { appTasks, OhosAppContext, OhosPluginId } from '@ohos/hvigor-ohos-plugin';
import { hvigor } from '@ohos/hvigor';

// 从环境变量注入发布签名 —— 不走 build-profile.json5 里的本地材料。
//
// 为什么不能直接把签名材料提交进 build-profile.json5:
//   1. 公开仓不能带密钥;仓里那份 signingConfigs 是空数组,本地那份靠
//      `git update-index --skip-worktree` 挡着(所以本机开 DevEco 照常能签)。
//   2. DevEco 写进 keyPassword/storePassword 的是它自己加密的密文,而这个加密
//      是平台相关的 —— macOS 上加密出来的串搬到 Linux CI 不保证能解。所以 CI
//      只能走明文密码 + 环境变量这条路。
//
// 需要的环境变量(缺任何一个就整个跳过,保持 build-profile.json5 原样):
//   FLEET_OHOS_STORE_FILE       .p12 密钥库
//   FLEET_OHOS_CERT_PATH        .cer 发布证书
//   FLEET_OHOS_PROFILE_PATH     .p7b 发布 Profile
//   FLEET_OHOS_KEY_ALIAS        密钥别名
//   FLEET_OHOS_STORE_PASSWORD   密钥库口令(明文)
//   FLEET_OHOS_KEY_PASSWORD     密钥口令(明文,缺省同 STORE_PASSWORD)
//   FLEET_OHOS_SIGN_ALG         缺省 SHA256withECDSA
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
    // 没打算用 env 签名(本机 DevEco 日常开发就是这条路),静默放过。
    return;
  }
  if (missing.length > 0) {
    // 半套材料比没有更危险:hvigor 对缺失的签名配置的反应是产出 *unsigned*
    // 包而不是报错,于是"我明明配了签名"会一路走到装不上/传不上才现形。
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

  // 光写 signingConfigs 是不够的 —— 每个 product 还要显式引用它。DevEco 自己
  // 就漏过这一步(2026-08-18 实测):材料在、引用不在,hvigor 报
  // "No signingConfig found for product default"、SignHap 空跑 2ms,只产出
  // entry-default-unsigned.hap。所以这里一并补上。
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
