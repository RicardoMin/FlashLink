// SPDX-License-Identifier: MIT
package com.limelight;

import android.content.Context;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.RouteInfo;
import android.os.SystemClock;

import java.net.DatagramPacket;
import java.io.IOException;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;

final class FlashLinkDiscovery {
    private static final int PORT = 47980;

    static List<Map<String, String>> scan(Context context) throws Exception {
        ConnectivityManager cm = (ConnectivityManager) context.getSystemService(Context.CONNECTIVITY_SERVICE);
        Set<Network> networks = new LinkedHashSet<>();
        Network active = cm.getActiveNetwork();
        if (active != null) networks.add(active);
        Collections.addAll(networks, cm.getAllNetworks());
        for (Network network : networks) {
            NetworkCapabilities capabilities = cm.getNetworkCapabilities(network);
            if (capabilities == null || !(capabilities.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                    || capabilities.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))) {
                continue;
            }
            LinkProperties link = cm.getLinkProperties(network);
            if (link != null && link.getInterfaceName() != null) {
                return scan(network, link);
            }
        }
        throw new IOException("No Wi-Fi or Ethernet network");
    }

    private static List<Map<String, String>> scan(Network network, LinkProperties link) throws Exception {
        Set<InetAddress> destinations = new LinkedHashSet<>();
        NetworkInterface nic = NetworkInterface.getByName(link.getInterfaceName());
        if (nic != null) {
            for (InterfaceAddress addr : nic.getInterfaceAddresses()) {
                if (addr.getBroadcast() != null) destinations.add(addr.getBroadcast());
            }
        }
        destinations.add(InetAddress.getByName("255.255.255.255"));
        // A Windows hotspot (and the Android emulator) may itself be the LAN gateway.
        for (RouteInfo route : link.getRoutes()) {
            if (route.isDefaultRoute() && route.getGateway() instanceof Inet4Address) {
                destinations.add(route.getGateway());
            }
        }
        String nonce = UUID.randomUUID().toString().replace("-", "");
        byte[] request = ("FLASHLINK_DISCOVER/1 " + nonce + "\n").getBytes(StandardCharsets.US_ASCII);
        Map<String, Map<String, String>> found = new LinkedHashMap<>();
        try (DatagramSocket socket = new DatagramSocket()) {
            network.bindSocket(socket);
            socket.setBroadcast(true);
            IOException sendError = null;
            boolean sent = false;
            for (InetAddress dest : destinations) {
                try {
                    socket.send(new DatagramPacket(request, request.length, dest, PORT));
                    sent = true;
                } catch (IOException e) {
                    sendError = e;
                }
            }
            if (!sent && sendError != null) throw sendError;
            long deadline = SystemClock.elapsedRealtime() + 1500;
            byte[] buffer = new byte[512];
            while (found.size() < 64) {
                long remaining = deadline - SystemClock.elapsedRealtime();
                if (remaining <= 0) break;
                socket.setSoTimeout((int) remaining);
                DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
                try { socket.receive(packet); } catch (SocketTimeoutException e) { break; }
                if (!(packet.getAddress() instanceof Inet4Address) || packet.getPort() != PORT
                        || packet.getLength() >= buffer.length) continue;
                String line = new String(buffer, 0, packet.getLength(), StandardCharsets.UTF_8);
                if (!line.endsWith("\n")) continue;
                String[] fields = line.substring(0, line.length() - 1).split(" ", 5);
                if (fields.length != 5 || !fields[0].equals("FLASHLINK_HOST/1") || !fields[1].equals(nonce)
                        || !fields[2].matches("[0-9]{1,5}") || !fields[3].matches("[0-9a-f]{64}")) continue;
                int port = Integer.parseInt(fields[2]);
                String name = fields[4].trim();
                if (port < 1032 || port > 65514 || name.isEmpty() || name.length() > 100
                        || fields[4].codePoints().anyMatch(c -> Character.isISOControl(c)
                        || Character.getType(c) == Character.FORMAT || c == 0xfffd)) continue;
                Map<String, String> pc = new LinkedHashMap<>();
                pc.put("id", fields[3]);
                pc.put("name", name);
                pc.put("host", packet.getAddress().getHostAddress() + ":" + port);
                found.putIfAbsent(fields[3], pc);
            }
        }
        return new ArrayList<>(found.values());
    }
}
