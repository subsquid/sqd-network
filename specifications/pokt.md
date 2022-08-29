# POKT Network

The Pocket Network is comprised of 3 components: Applications, Nodes and the Network Layer.

- An Application submits Relays, or API requests meant to be routed to any public database node.
- Nodes service these Relays, by submitting them to the public databases they are meant for, and sending the response (if any) back to the Application.
- The Network Layer is comprised of all the rules, protocols and finality storage that serve as the backbone of the interactions between Applications and Nodes, including (but not limited to), configuration, record tracking, governance and economic policy.

## Session

The mechanism the Network uses to regulate the interactions between Applications and Nodes are Sessions.

Session generation is based on the app's public key and latest block to provide more random value. The final nodes are selected based on minimal distance between the session key and the node itself.

## Application

An Application is any software application, registered on the Pocket Network as such, which submits Relays meant to be routed to any public database node. The rate at which these Relays can be sent through the network is determined by the Application allocated throughput, which is consistent Network-wide. Applications are responsible for the validation of their own data integrity, and can enforce security via cross-check verification of results from multiple Nodes in the network. This cross-check mechanism is called Client-Side Validation.

### Allocated Throughput

The only way to obtain sanctioned throughput from Nodes within the Pocket Network is by staking the native cryptocurrency within the network.

### Throughput utilization and service patterns

Due to the architecture of the Session mechanism, Applications are provided five (5) individual Service Nodes per requested chain. The way the Applications allocate their throughput is up to their sole discretion. Though there are many configurations, there are two recommended and anticipated patterns the Applications should be aware of when designing their clients.

- Optimization for speed: each Application designed under this pattern will send distinct Relay requests to each individual Node. In this pattern, there is no Client-Side Validation, but by parallelizing the requests, the Application is able to do far more requests than the security optimization pattern.
- Optimization for security: the next Application service pattern described in this document is an optimization for security. In this model, each Application sends the same request to all (5) Nodes. Once the responses from each Node are collected, the Application can perform Client-Side Validation by selecting the answer with the highest majority. Though this is expected to be significantly slower than the speed optimization pattern, the data security guarantees are much higher.

### Application authorization token

An Application signed token is needed by each client to authorize the use of the Application throughput. Similar to the Service Patterns, there are two basic design patterns for AAT distribution that are recommended and anticipated: optimize for safety, optimize for performance and UX.

### Client-side validation

In order to ensure the truthfulness of the data received by nodes in the Pocket Network, Applications have the choice to use their allocated throughput to cross check the results and assume that the majority of nodes that return consistent results are being truthful while the minority is lying. If they choose to, Applications can submit a “Challenge Transaction” to the Pocket Network using the results of the cross check as proof in order to burn the lying Nodes.

### Application Optimization

Though each Application is able to customize the design of their Decentralized Application as they see fit like session management and dispatch routing table.

## Nodes

At its core, a Pocket Network Node is an ABCI Application, which means it is a Tendermint Core compatible state machine. At a high level, this state machine can be broken down into 3 core components:

1. Dispatch: ​Serves Session information to an Application, serving as the first point of contact between it and the Nodes that will service it.
2. Service: ​Services Relays submitted by Applications, if they belong to the corresponding Session.
3. Finality Storage: ​Stores the Pocket Network state, including account balances, Application and Node records, and contains the business logic to validate work reports submitted by Nodes.

### Dispatch Nodes

Dispatch Nodes, more traditionally known as edge nodes in distributed systems, are Nodes on the network that provide the Dispatch Service to Applications. It is important to note that every active node registered for service (via staking the native cryptocurrency) on the network is a Dispatch Node. Dispatch Nodes serve two functions to Applications:

- Session request. The Dispatch Node runs the Session Generation Algorithm for the specific Application Client and returns the Session information to the client. It is important to note that every session is predetermined pseudorandomly by the seed data passed from the Application Client, meaning that every actor on the network can generate the same session if given the same seed data (using the Finality Storage as the consistency guarantee). This allows each client to know who their Service Nodes are by connecting with any active Dispatch Node on the network.
- Peer sharing. This routing table increases both network decentralization and application security, as they can go to multiple nodes to find out their session information.

### Service Nodes

Nodes that service the Relays of the Application clients through the Pocket Network are known as Service Nodes. Service Nodes must run at least one full non-native blockchain node to provide up-to-date information to the Application Clients upon request.

### Relay batch lifecycle

In the context of the Pocket Network, a relay is a Non-Native Blockchain API request and response transmitted through the Pocket Network.

1. Service Node computes the Relay Batch data structure.
2. Service Node sends the Relay Batch as a transaction to the Finality Storage layer.
3. Transaction gets picked up by Leader from the Finality Storage mempool, and gets validated according to the Relay Batch Evidence Selection Algorithm.
4. One of two outcomes can happen depending on the validation result:
   - The transaction is valid: the protocol automatically mints new native cryptocurrency corresponding to the applicable monetary policy of the network.
   - The transaction is invalid: If the transaction doesn’t match the expected result of the Relay Batch Evidence Selection Algorithm, the responsible Service Node gets burned and/or punished according to the applicable monetary policy of the network and other network rules.
