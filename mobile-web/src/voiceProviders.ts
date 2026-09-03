// provider 注册表：把 detectVoiceProvider() 判出的环境映射到具体实现。
//
// 单独一个模块是为了让 useVoiceInput 不必 import 三条实现 —— 那会把 Capacitor
// 插件和鸿蒙桥的代码拖进浏览器包里。这里同样只在被选中时才碰对应实现。
//
// 目前只登记了 Web Speech；Capacitor 与鸿蒙两条在各自的 P-task 里接进来，届时
// 只需在 REGISTRY 里加一行，调用方不用改。

import { detectVoiceProvider, type VoiceInputProvider, type VoiceProviderId } from "./voiceInput";
import { capacitorVoiceProvider } from "./voiceCapacitor";
import { harmonyVoiceProvider } from "./voiceHarmony";
import { webSpeechProvider } from "./voiceWebSpeech";

const REGISTRY: Partial<Record<VoiceProviderId, VoiceInputProvider>> = {
  "web-speech": webSpeechProvider,
  capacitor: capacitorVoiceProvider,
  harmony: harmonyVoiceProvider,
};

/** 当前环境该用的 provider，没有可用实现时 null。 */
export function currentVoiceProvider(): VoiceInputProvider | null {
  const id = detectVoiceProvider();
  return id ? (REGISTRY[id] ?? null) : null;
}
