//! Détection des adresses IP locales / interfaces réseau, pour la section
//! « Depuis un autre appareil ».

use std::net::IpAddr;

/// Retourne la liste des adresses IPv4 locales « routables sur le LAN »
/// (exclut loopback et link-local), triées et dédupliquées.
///
/// Utilisé pour proposer à l'utilisateur les URLs à taper depuis une autre
/// machine du réseau.
pub fn local_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = Vec::new();

    if let Ok(list) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in list {
            if is_lan_ipv4(&ip) {
                ips.push(ip);
            }
        }
    }

    ips.sort_by_key(|ip| ip.to_string());
    ips.dedup();
    ips
}

/// Vrai si l'IP est une IPv4 exploitable sur le réseau local (ni loopback,
/// ni link-local, ni non-spécifiée, ni broadcast).
fn is_lan_ipv4(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            !v4.is_loopback()
                && !v4.is_link_local()
                && !v4.is_unspecified()
                && !v4.is_broadcast()
                && !v4.is_multicast()
        }
        IpAddr::V6(_) => false,
    }
}
