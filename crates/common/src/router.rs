use std::net::Ipv4Addr;
use log::debug;
use serde::{Deserialize, Serialize};
use crate::messages::EndpointId;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Route {
    pub address: Ipv4Addr,
    pub subnet_mask: Ipv4Addr
}

impl Route {
    fn reachable_from(&self, address_to_test: &Ipv4Addr) -> bool {

        return if self.calculate_network_address() == (address_to_test & self.subnet_mask) {
            true
        } else {
            false
        }
    }

    fn calculate_network_address(&self) -> Ipv4Addr {
        self.address & self.subnet_mask
    }
}

#[derive(PartialEq, Debug)]
struct AddressEndpointMapping {
    address: Route,
    endpoint_id: EndpointId
}

#[derive(Debug)]
pub struct Router {
    routes: Vec<AddressEndpointMapping>
}

impl Router {
    pub fn new() -> Self {
        Router { routes: Vec::new() }
    }

    pub fn insert_route(&mut self, address: Route, endpoint_id: EndpointId) {
        debug!("Learning about ep: {} - {:?}", endpoint_id, address);
        let aem = AddressEndpointMapping {
            address,
            endpoint_id
        };

        if !self.routes.contains(&aem) {
            self.routes.push(aem)
        }

        // Ensure default route (0.0.0.0/0) is the last in the list
        // TODO: Ensure this is an adequate way of doing it
        let default_route = Ipv4Addr::new(0,0,0,0);
        let index_of_default_route = self.routes.iter().position(|n| n.address.address == default_route && n.address.subnet_mask == default_route);

        if let Some(index) = index_of_default_route {
            if index != self.routes.len() - 1 {
                let last_index = self.routes.len() - 1;
                self.routes.swap(index, last_index)
            }
        }

        debug!("routing table: {:?}", self.routes)
    }

    pub fn lookup(&self, address_to_lookup: &Ipv4Addr) -> Vec<EndpointId> {
        let mut endpoints = Vec::new();

        for aem in &self.routes {
            if aem.address.reachable_from(&address_to_lookup) {
                if !endpoints.contains(&aem.endpoint_id) {
                    endpoints.push(aem.endpoint_id)
                }
            }
        }

        endpoints
    }

    pub fn remove_route_to_endpoint(&mut self, endpoint_id: EndpointId) {
        self.routes.retain(|aem| aem.endpoint_id != endpoint_id)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_is_in_subnet() {
        let a = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };

        let b = Ipv4Addr::new(192, 168, 0, 2);

        assert_eq!(a.reachable_from(&b), true)
    }

    #[test]
    fn address_is_in_subnet_wider_mask() {
        let a = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 0, 0),
        };

        let b = Ipv4Addr::new(192, 168, 0, 2);

        assert_eq!(a.reachable_from(&b), true)
    }

    #[test]
    fn address_is_in_subnet_wider_mask2() {
        let a = Route {
            address: Ipv4Addr::new(192, 168, 100, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 0, 0),
        };

        let b = Ipv4Addr::new(192, 168, 0, 2);

        assert_eq!(a.reachable_from(&b), true)
    }

    #[test]
    fn address_is_not_in_subnet() {
        let a = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };

        let b = Ipv4Addr::new(192, 168, 100, 2);

        assert_eq!(a.reachable_from(&b), false)
    }

    #[test]
    fn insert_route_adds_new_route() {
        let mut router = Router { routes: Vec::new() };
        let address = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let endpoint_id = 1;

        router.insert_route(address, endpoint_id);

        assert_eq!(router.routes.len(), 1);
    }

    #[test]
    fn insert_route_does_not_add_duplicate_route() {
        let mut router = Router { routes: Vec::new() };
        let address = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let endpoint_id = 1;

        router.insert_route(address.clone(), endpoint_id);
        router.insert_route(address, endpoint_id);

        assert_eq!(router.routes.len(), 1);
    }

    #[test]
    fn lookup_returns_correct_endpoint() {
        let mut router = Router { routes: Vec::new() };
        let address = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let endpoint_id = 1;

        router.insert_route(address.clone(), endpoint_id);

        let result = router.lookup(&Ipv4Addr::new(192, 168, 0, 5));

        assert_eq!(result, vec![endpoint_id]);
    }

    #[test]
    fn lookup_returns_empty_vector_for_unknown_address() {
        let router = Router { routes: Vec::new() };

        let result = router.lookup(&Ipv4Addr::new(192, 168, 0, 5));

        assert_eq!(result, Vec::<EndpointId>::new());
    }

    #[test]
    fn lookup_returns_multiple_endpoints_for_same_address() {
        let mut router = Router { routes: Vec::new() };
        let address = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let endpoint_id1 = 1;
        let endpoint_id2 = 2;

        router.insert_route(address.clone(), endpoint_id1);
        router.insert_route(address.clone(), endpoint_id2);

        let result = router.lookup(&Ipv4Addr::new(192, 168, 0, 5));

        assert_eq!(result, vec![endpoint_id1, endpoint_id2]);
    }

    #[test]
    fn insert_and_route_to_subnet() {
        let mut router = Router { routes: Vec::new() };
        let subnet_address = Route {
            address: Ipv4Addr::new(192, 168, 0, 0),
            subnet_mask: Ipv4Addr::new(255, 255, 0, 0),
        };
        let endpoint_id = 1;

        router.insert_route(subnet_address.clone(), endpoint_id);

        let result = router.lookup(&Ipv4Addr::new(192, 168, 0, 5));

        assert_eq!(result, vec![endpoint_id]);
    }

    #[test]
    fn insert_and_route_to_subnet_with_multiple_endpoints() {
        let mut router = Router { routes: Vec::new() };
        let subnet_address = Route {
            address: Ipv4Addr::new(192, 168, 0, 1),
            subnet_mask: Ipv4Addr::new(255, 255, 0, 0),
        };
        let endpoint_id1 = 1;
        let endpoint_id2 = 2;

        router.insert_route(subnet_address.clone(), endpoint_id1);
        router.insert_route(subnet_address.clone(), endpoint_id2);

        let result = router.lookup(&Ipv4Addr::new(192, 168, 0, 5));

        assert_eq!(result, vec![endpoint_id1, endpoint_id2]);
    }

    #[test]
    fn insert_and_route_to_subnet_with_no_matching_endpoint() {
        let mut router = Router { routes: Vec::new() };
        let subnet_address = Route {
            address: Ipv4Addr::new(192, 168, 100, 0),
            subnet_mask: Ipv4Addr::new(255, 255, 255, 0),
        };
        let endpoint_id = 1;

        router.insert_route(subnet_address, endpoint_id);


        let result = router.lookup(&Ipv4Addr::new(192, 168, 0, 5));

        assert_eq!(result, Vec::<EndpointId>::new());
    }
}