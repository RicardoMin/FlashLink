// Sunshine v2026.914.233613 binds base + AUDIO_STREAM_PORT in start_broadcast()
// and releases the shared broadcast context when the last media session ends.
#include <winsock2.h>
#include <ws2tcpip.h>
#include <iphlpapi.h>
#include <vector>

namespace {
template <typename Table>
int owns_udp_port(ULONG family, DWORD pid, unsigned short port) {
    // DWORD storage gives both IP Helper table layouts their required alignment.
    std::vector<DWORD> storage(128);
    for (;;) {
        DWORD bytes = static_cast<DWORD>(storage.size() * sizeof(DWORD));
        DWORD result = GetExtendedUdpTable(storage.data(), &bytes, FALSE, family, UDP_TABLE_OWNER_PID, 0);
        if (result == ERROR_INSUFFICIENT_BUFFER) {
            storage.resize((bytes + sizeof(DWORD) - 1) / sizeof(DWORD));
            continue;
        }
        if (result != NO_ERROR) return -1;
        const auto* table = reinterpret_cast<const Table*>(storage.data());
        for (DWORD i = 0; i < table->dwNumEntries; ++i) {
            const auto& row = table->table[i];
            if (row.dwOwningPid == pid && ntohs(static_cast<u_short>(row.dwLocalPort)) == port) return true;
        }
        return false;
    }
}
}

extern "C" BOOL flashlink_media_active(DWORD engine_pid, unsigned short audio_port) {
    return owns_udp_port<MIB_UDPTABLE_OWNER_PID>(AF_INET, engine_pid, audio_port) == 1
        || owns_udp_port<MIB_UDP6TABLE_OWNER_PID>(AF_INET6, engine_pid, audio_port) == 1;
}

// Updating requires positive confirmation that the transport is idle.
extern "C" BOOL flashlink_media_idle(DWORD engine_pid, unsigned short audio_port) {
    return owns_udp_port<MIB_UDPTABLE_OWNER_PID>(AF_INET, engine_pid, audio_port) == 0
        && owns_udp_port<MIB_UDP6TABLE_OWNER_PID>(AF_INET6, engine_pid, audio_port) == 0;
}
