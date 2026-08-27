#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <iomanip>
#include <iostream>
#include <string>
#include <thread>
#include <vector>

#if defined(_WIN32)
#define NOMINMAX
#include <windows.h>
#include <psapi.h>
#pragma comment(lib, "Psapi.lib")
#elif defined(__APPLE__) || defined(__linux__)
#include <sys/resource.h>
#endif

#include "sherpa-onnx/c-api/cxx-api.h"

namespace {

using Clock = std::chrono::steady_clock;

double elapsed_ms(Clock::time_point start) {
  return std::chrono::duration<double, std::milli>(Clock::now() - start).count();
}

std::string json_escape(const std::string &value) {
  std::string escaped;
  escaped.reserve(value.size() + 16);
  for (const unsigned char ch : value) {
    switch (ch) {
      case '\\':
        escaped += "\\\\";
        break;
      case '"':
        escaped += "\\\"";
        break;
      case '\n':
        escaped += "\\n";
        break;
      case '\r':
        escaped += "\\r";
        break;
      case '\t':
        escaped += "\\t";
        break;
      default:
        if (ch < 0x20) {
          escaped += "?";
        } else {
          escaped += static_cast<char>(ch);
        }
    }
  }
  return escaped;
}

int64_t peak_rss_bytes() {
#if defined(_WIN32)
  PROCESS_MEMORY_COUNTERS counters {};
  if (!GetProcessMemoryInfo(GetCurrentProcess(), &counters, sizeof(counters))) {
    return -1;
  }
  return static_cast<int64_t>(counters.PeakWorkingSetSize);
#elif defined(__APPLE__) || defined(__linux__)
  struct rusage usage {};
  if (getrusage(RUSAGE_SELF, &usage) != 0) {
    return -1;
  }
#if defined(__APPLE__)
  return usage.ru_maxrss;
#else
  return static_cast<int64_t>(usage.ru_maxrss) * 1024;
#endif
#else
  return -1;
#endif
}

double process_cpu_ms() {
#if defined(_WIN32)
  FILETIME created {};
  FILETIME exited {};
  FILETIME kernel {};
  FILETIME user {};
  if (!GetProcessTimes(GetCurrentProcess(), &created, &exited, &kernel, &user)) {
    return -1;
  }
  ULARGE_INTEGER kernel_ticks {};
  kernel_ticks.LowPart = kernel.dwLowDateTime;
  kernel_ticks.HighPart = kernel.dwHighDateTime;
  ULARGE_INTEGER user_ticks {};
  user_ticks.LowPart = user.dwLowDateTime;
  user_ticks.HighPart = user.dwHighDateTime;
  return static_cast<double>(kernel_ticks.QuadPart + user_ticks.QuadPart) / 10000.0;
#elif defined(__APPLE__) || defined(__linux__)
  struct rusage usage {};
  if (getrusage(RUSAGE_SELF, &usage) != 0) {
    return -1;
  }
  const double user_ms = usage.ru_utime.tv_sec * 1000.0 + usage.ru_utime.tv_usec / 1000.0;
  const double system_ms =
      usage.ru_stime.tv_sec * 1000.0 + usage.ru_stime.tv_usec / 1000.0;
  return user_ms + system_ms;
#else
  return -1;
#endif
}

void usage(const char *program) {
  std::cerr << "usage: " << program
            << " <runtime-model-dir> <audio.wav> [chunk-ms] [num-threads]"
               " [--content-free]\n";
}

}  // namespace

int main(int argc, char **argv) {
  if (argc < 3 || argc > 6) {
    usage(argv[0]);
    return 2;
  }

  const std::string model_dir = argv[1];
  const std::string audio_path = argv[2];
  const int chunk_ms = argc >= 4 ? std::atoi(argv[3]) : 120;
  const int num_threads = argc >= 5 ? std::atoi(argv[4]) : 2;
  const bool content_free = argc == 6 && std::string(argv[5]) == "--content-free";
  if (chunk_ms <= 0 || num_threads <= 0 || (argc == 6 && !content_free)) {
    usage(argv[0]);
    return 2;
  }

  const auto wave = sherpa_onnx::cxx::ReadWave(audio_path);
  if (wave.samples.empty() || wave.sample_rate <= 0) {
    std::cerr << "failed to read mono PCM WAVE input\n";
    return 3;
  }

  sherpa_onnx::cxx::OnlineRecognizerConfig config;
  config.model_config.transducer.encoder = model_dir + "/encoder.int8.onnx";
  config.model_config.transducer.decoder = model_dir + "/decoder.int8.onnx";
  config.model_config.transducer.joiner = model_dir + "/joiner.int8.onnx";
  config.model_config.tokens = model_dir + "/tokens.txt";
  config.model_config.num_threads = num_threads;
  config.model_config.provider = "cpu";
  config.decoding_method = "greedy_search";
  config.enable_endpoint = false;

  const auto init_start = Clock::now();
  auto recognizer = sherpa_onnx::cxx::OnlineRecognizer::Create(config);
  if (recognizer.Get() == nullptr) {
    std::cerr << "failed to create online recognizer\n";
    return 4;
  }
  const double initialization_ms = elapsed_ms(init_start);
  auto stream = recognizer.CreateStream();
  if (stream.Get() == nullptr) {
    std::cerr << "failed to create online stream\n";
    return 5;
  }

  // sherpa-onnx's reference file decoder supplies 500 ms of left padding.
  // Pre-roll the cache before starting the wall-clock measurement so a live
  // stream does not lose speech that begins immediately.
  std::vector<float> left_padding(wave.sample_rate / 2, 0.0f);
  stream.AcceptWaveform(wave.sample_rate, left_padding.data(),
                        static_cast<int32_t>(left_padding.size()));
  while (recognizer.IsReady(&stream)) {
    recognizer.Decode(&stream);
  }

  const int64_t chunk_samples = std::max<int64_t>(
      1, static_cast<int64_t>(wave.sample_rate) * chunk_ms / 1000);
  const auto stream_start = Clock::now();
  std::string last_text;
  double first_update_ms = -1;
  double last_update_ms = -1;
  double max_update_gap_ms = 0;
  int updates = 0;
  int revisions = 0;

  auto collect_result = [&]() {
    const auto result = recognizer.GetResult(&stream);
    if (result.text.empty() || result.text == last_text) {
      return;
    }
    const double now_ms = elapsed_ms(stream_start);
    if (first_update_ms < 0) {
      first_update_ms = now_ms;
    }
    if (last_update_ms >= 0) {
      max_update_gap_ms = std::max(max_update_gap_ms, now_ms - last_update_ms);
      ++revisions;
    }
    last_update_ms = now_ms;
    ++updates;
    last_text = result.text;
    if (!content_free) {
      std::cerr << std::fixed << std::setprecision(1) << "update_ms=" << now_ms
                << " text=" << result.text << '\n';
    }
  };

  int64_t offset = 0;
  while (offset < static_cast<int64_t>(wave.samples.size())) {
    const int64_t count = std::min<int64_t>(
        chunk_samples, static_cast<int64_t>(wave.samples.size()) - offset);
    const double target_ms =
        1000.0 * static_cast<double>(offset + count) / wave.sample_rate;
    std::this_thread::sleep_until(
        stream_start + std::chrono::duration_cast<Clock::duration>(
                           std::chrono::duration<double, std::milli>(target_ms)));
    stream.AcceptWaveform(wave.sample_rate, wave.samples.data() + offset,
                          static_cast<int32_t>(count));
    while (recognizer.IsReady(&stream)) {
      recognizer.Decode(&stream);
      collect_result();
    }
    offset += count;
  }

  const std::string text_at_audio_end = last_text;
  // Match sherpa-onnx's file decoder: give the streaming encoder 800 ms of
  // right context at end-of-input. Feed it immediately because this is stop
  // finalization, not additional recorded time.
  const int32_t right_padding_samples = wave.sample_rate * 8 / 10;
  std::vector<float> right_padding(right_padding_samples, 0.0f);
  stream.AcceptWaveform(wave.sample_rate, right_padding.data(),
                        right_padding_samples);
  stream.InputFinished();
  while (recognizer.IsReady(&stream)) {
    recognizer.Decode(&stream);
    collect_result();
  }
  collect_result();

  const double completion_ms = elapsed_ms(stream_start);
  const double audio_ms =
      1000.0 * static_cast<double>(wave.samples.size()) / wave.sample_rate;

  std::cout << std::fixed << std::setprecision(1)
            << "{\"engine\":\"sherpa-nemotron-streaming\","
            << "\"chunkMs\":" << chunk_ms << ","
            << "\"threads\":" << num_threads << ","
            << "\"audioMs\":" << audio_ms << ","
            << "\"initializationMs\":" << initialization_ms << ","
            << "\"firstUpdateMs\":" << first_update_ms << ","
            << "\"updates\":" << updates << ","
            << "\"revisions\":" << revisions << ","
            << "\"maxUpdateGapMs\":" << max_update_gap_ms << ","
            << "\"completionMs\":" << completion_ms << ","
            << "\"completionLagMs\":" << completion_ms - audio_ms << ","
            << "\"processCpuMs\":" << process_cpu_ms() << ","
            << "\"peakRssBytes\":" << peak_rss_bytes();
  if (!content_free) {
    std::cout << ",\"textAtAudioEnd\":\"" << json_escape(text_at_audio_end)
              << "\",\"finalText\":\"" << json_escape(last_text) << "\"";
  }
  std::cout << "}\n";
  return last_text.empty() ? 6 : 0;
}
