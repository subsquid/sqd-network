# Archive decentralization

## Abstract

## Subsquide team goals

## Similar approaches

### POKT Network

The Pocket Network is comprised of 3 components: Applications, Nodes and the Network Layer.

- An Application submits Relays, or API requests meant to be routed to any public database node.
- Nodes service these Relays, by submitting them to the public databases they are meant for, and sending the response (if any) back to the Application.
- The Network Layer is comprised of all the rules, protocols and finality storage that serve as the backbone of the interactions between Applications and Nodes, including (but not limited to), configuration, record tracking, governance and economic policy.

#### Session

The mechanism the Network uses to regulate the interactions between Applications and Nodes are Sessions.

Session generation is based on the app's public key and latest block to provide more random value. The final nodes are selected based on minimal distance between the session key and the node itself.

#### Application

An Application is any software application, registered on the Pocket Network as such, which submits Relays meant to be routed to any public database node. The rate at which these Relays can be sent through the network is determined by the Application allocated throughput, which is consistent Network-wide. Applications are responsible for the validation of their own data integrity, and can enforce security via cross-check verification of results from multiple Nodes in the network. This cross-check mechanism is called Client-Side Validation.

**Allocated Throughput**

The only way to obtain sanctioned throughput from Nodes within the Pocket Network is by staking the native cryptocurrency within the network.

**Throughput utilization and service patterns**

Due to the architecture of the Session mechanism, Applications are provided five (5) individual Service Nodes per requested chain. The way the Applications allocate their throughput is up to their sole discretion. Though there are many configurations, there are two recommended and anticipated patterns the Applications should be aware of when designing their clients.

- Optimization for speed: each Application designed under this pattern will send distinct Relay requests to each individual Node. In this pattern, there is no Client-Side Validation, but by parallelizing the requests, the Application is able to do far more requests than the security optimization pattern.
- Optimization for security: the next Application service pattern described in this document is an optimization for security. In this model, each Application sends the same request to all (5) Nodes. Once the responses from each Node are collected, the Application can perform Client-Side Validation by selecting the answer with the highest majority. Though this is expected to be significantly slower than the speed optimization pattern, the data security guarantees are much higher.

**Application authorization token**

An Application signed token is needed by each client to authorize the use of the Application throughput. Similar to the Service Patterns, there are two basic design patterns for AAT distribution that are recommended and anticipated: optimize for safety, optimize for performance and UX.

**Client-side validation**

In order to ensure the truthfulness of the data received by nodes in the Pocket Network, Applications have the choice to use their allocated throughput to cross check the results and assume that the majority of nodes that return consistent results are being truthful while the minority is lying. If they choose to, Applications can submit a “Challenge Transaction” to the Pocket Network using the results of the cross check as proof in order to burn the lying Nodes.

**Application Optimization**

Though each Application is able to customize the design of their Decentralized Application as they see fit like session management and dispatch routing table.

#### Nodes

At its core, a Pocket Network Node is an ABCI Application, which means it is a Tendermint Core compatible state machine. At a high level, this state machine can be broken down into 3 core components:

1. Dispatch: ​Serves Session information to an Application, serving as the first point of contact between it and the Nodes that will service it.
2. Service: ​Services Relays submitted by Applications, if they belong to the corresponding Session.
3. Finality Storage: ​Stores the Pocket Network state, including account balances, Application and Node records, and contains the business logic to validate work reports submitted by Nodes.

**Dispatch Nodes**

Dispatch Nodes, more traditionally known as edge nodes in distributed systems, are Nodes on the network that provide the Dispatch Service to Applications. It is important to note that every active node registered for service (via staking the native cryptocurrency) on the network is a Dispatch Node. Dispatch Nodes serve two functions to Applications:

- Session request. The Dispatch Node runs the Session Generation Algorithm for the specific Application Client and returns the Session information to the client. It is important to note that every session is predetermined pseudorandomly by the seed data passed from the Application Client, meaning that every actor on the network can generate the same session if given the same seed data (using the Finality Storage as the consistency guarantee). This allows each client to know who their Service Nodes are by connecting with any active Dispatch Node on the network.
- Peer sharing. This routing table increases both network decentralization and application security, as they can go to multiple nodes to find out their session information.

**Service Nodes**

Nodes that service the Relays of the Application clients through the Pocket Network are known as Service Nodes. Service Nodes must run at least one full non-native blockchain node to provide up-to-date information to the Application Clients upon request.

**Relay batch lifecycle**

In the context of the Pocket Network, a relay is a Non-Native Blockchain API request and response transmitted through the Pocket Network.

1. Service Node computes the Relay Batch data structure.
2. Service Node sends the Relay Batch as a transaction to the Finality Storage layer.
3. Transaction gets picked up by Leader from the Finality Storage mempool, and gets validated according to the Relay Batch Evidence Selection Algorithm.
4. One of two outcomes can happen depending on the validation result:
   - The transaction is valid: the protocol automatically mints new native cryptocurrency corresponding to the applicable monetary policy of the network.
   - The transaction is invalid: If the transaction doesn’t match the expected result of the Relay Batch Evidence Selection Algorithm, the responsible Service Node gets burned and/or punished according to the applicable monetary policy of the network and other network rules.

### The Graph

Graph Protocol falls into a category as a layer 2 read-scalability solution.

#### High-Level Architecture

```ascii
    +------------------------------------------------------------------+
    |                                                                  |
    | Decentralized Application (dApp)                                 |
    |                                                                  |
    +-+---------------------------------^--------------------------+---+
      |                                 |                          |
      |                              Queries                       |
      |                                 |                          |
      |   +-----------------------------+--------------------+     |
      |   |                                                  |     |
      |   |  Query Nodes and Clients                         | Micropayments
      |   |                                                  |     |
      |   +---------+-------------------^--------------------+     |
      |             |                   |                          |
Transactions   Attestations     (Reads, Attestations)              |
      |             |                   |                          |
      |   +---------v-----------+  +----+--------------------------v---+
      |   |                     |  |                                   |
      |   |  Fisherman Service  |  | Indexing Nodes                    |
      |   |                     |  |                                   |
      |   +---------+-----------+  +----^-------------------^----------+
      |             |                   |                   |
      |         Disputes          (Events, Data)          Data
      |             |                   |                   |
    +-v-------------v-------------------+-----+ +-----------+----------+
    |                                         | |                      |
    |                Ethereum                 | |   IPFS               |
    |                                         | |                      |
    +-----------------------------------------+ +----------------------+
```

**Query [Nodes | Clients]**

Query Nodes provide an abstraction on top of the low-level read API provided by the Indexing Nodes. In addition to providing an interface to dApps, the Query Node is responsible for discovering Indexing Nodes in the network that are indexing a specific dataset, and selecting an Indexing Node to read from based on factors such as price and performance (see Query Processing). It may also optionally forward attestations along to a Fisherman Service.

**Indexing nodes**

Indexing Nodes index one or more user-defined datasets, called subgraphs. These nodes perform a deterministic streaming extract, transform and load (ETL) of events emitted by the Ethereum blockchain. These events are processed by user-defined logic called mappings which run deterministically inside a WASM runtime, and are also able to load additional data from the Ethereum blockchain or IPFS, in order to compute the current state of a subgraph.

**Fisherman service**

Fisherman Services accept read responses and attestations which they may verify, and in the event of an invalid response, may file a protocol-level dispute.

#### Mechanism design

Indexing Node operators stake deposits of Graph Tokens for particular datasets, called subgraphs, to gain the right to participate in the data retrieval marketplaces for that dataset-indexing data and responding to read requests in exchange for micropayments. This deposit is forfeit in the event that the work is not performed correctly, or is performed maliciously, as defined in the slashing conditions.

**Graph Token**

Graph Tokens, which are the only token that may be used for staking in the network. However, ETH or DAI is used for paying for read operations, thus reducing friction and balance sheet risk for end-users of dApps that query The Graph.

**Staking**

Indexing Nodes deposit a `stakingAmount` of Graph Tokens to process read requests for a specific dataset, which is identified by its `subgraphID`. The stakingAmount must be in the set of the top N staking amounts, where N is determined by the maxIndexers parameter that is set via governance.

Indexing Nodes that have staked for a dataset are not limited by the protocol in how many read requests they may process for that dataset. However, it may be assumed that Indexing Nodes with higher deposits will receive more read requests and, thus, collect more fees, if all else is equal, as this represents a greater economic security margin to the end user.

**Data Retrieval Market**

Indexing Nodes which have staked to index a particular dataset, will be discoverable in the data retrieval market for that dataset.

Indexing Nodes receive requests which include a Read Operation and a Locked Transfer.
The Read Operation fully defines the data that is being requested, while the Locked Transfer is a micropayment that is paid, conditional, on the Indexing Node producing a Read Response along with a signed Attestation message which certifies the response data is correct.

**Data Retrieval Pricing**

Pricing in the data retrieval market is set according to the bandwidth and compute required to process a request.

- Compute is priced as a gasPrice, denominated in ETH or DAI, where the gas required for a request is determined by the specific read operation and parameters. See Read Interface for operation specific gas prices.
- Bandwidth is priced in bytesPrice, denominated in ETH or DAI, where bytes refers to the size of the data portion of the response, measured in bytes.

Indexing Nodes respond with their compute and bandwidth costs in response to the getPrices method in the JSON-RPC API.

**Verification**

A Fisherman Service is an economic agent who verifies read responses in exchange for a reward in cases where they detect that an Indexing Node has attested to an incorrect response, and the Fisherman successfully disputes the response on-chain.

**Curation Market**

Curators are economic agents who earn rewards by betting on the future economic value of datasets, perhaps with the benefit of private information.

A Curator stakes a deposit of Graph Tokens for a particular dataset in exchange for dataset-specific subgraph tokens. These tokens entitle the holder to a portion of a curation reward, which is paid in Graph Tokens through inflation. See Inflation Rewards for how curation reward is calculated for each dataset.

**Stake delegation**

Token holders who do not feel equipped to perform one of these functions may delegate their tokens to an Indexing Node that is staked for a particular dataset. In this case, the delegator is the residual claimant for their stake, earning participation rewards according to the activities of the delegatee Indexing Node but also forfeiting their stake in the event that the delagatee Indexing Node is slashed.

#### Query processing architecture

In either construction, query processing consists of the following steps:
1. Query Planning (Optional)
2. Service Discovery
3. Service Selection
4. Processing and Payment
5. Response Collation

**Query Planning**

In this stage, the Query Node transforms a query into a plan, consisting of an ordered set of lower-level read operations that may be used to retrieve the data specified by the query.

**Service Discovery**

Processing a query plan, or processing a query directly, results in low-level read operations being made to Indexing Nodes. Each read operation corresponds to a specific dataset and, thus, needs to be made against an Indexing Node for that dataset. In the Service Discovery step, the Query Node locates Indexing Nodes for a specific dataset as well as important metadata that is useful in deciding which Indexing Node to issue read operations to, such as price, performance, and economic security margin.

Fetching price and latency for a node is done via a single call to the Indexing Node RPC API and returns the following data: the latency required to fulfill the request; a `bandwidthPrice` measured in price per byte transmitted over the network; and a `gasPrice`, which captures the cost of compute and IO for a given read operation.

**Service Selection**

In the Service Selection stage, Query Nodes choose which Indexing Nodes to transact with for each read operation. An algorithm for this stage could incorporate latency (measured in ms), economicSecurityMargin (measured in Graph Tokens), gasPrice, and bytesPrice (the cost of sending a byte over the network).

**Processing and Payment**

Available read operations are defined in the Read Interface, and are sent to the Indexing Nodes via the JSON-RPC API. They are accompanied by Locked Transfers, conditional micropayments that may be unlocked by the Indexing Node producing a Read Response and a signed Attestation message certifying the response data is correct.

### Kyve

KYVE is a network of storage pools built to store data streams or create snapshots of already existing data. It is secured by its blockchain built on cosmos.

#### Architecture

![image](https://docs.kyve.network/architecture.png)

**Chain Layer**

The chain layer is the backbone of KYVE. The chain layer is an entirely sovereign Proof of Stake. This blockchain is run by independent nodes we call chain nodes since they're running on the chain level. The native currency of the KYVE chain is $KYVE. It secures the chain and allows chain nodes to stake and other users to delegate to them.

**Protocol Layer**

The protocol layer sits on top of the chain layer and enables the actual use case of KYVE. Every feature and unit of logic which makes KYVE unique is implemented directly into the chain nodes. This includes pools, funding, staking and delegating.

#### $KYVE

$KYVE is the native currency of the KYVE blockchain. On a chain level, $KYVE is used for staking and delegating, securing the network through Proof of Stake. Furthermore, $KYVE is used on the protocol level for funding, staking, and delegating, incentivizing participants to behave accordingly.

#### Pools

Generally, storage pools (or just pools) can be described as discrete entities arranged around specific data sources. Anyone can create them through governance and can store any data stream.

Protocol nodes have to run with the specified pool runtime for a pool to function. If specific criteria are met, pools distribute $KYVE to designated node runners.

A pool always requires two instructions:
- How to retrieve data from a data source
- How to validate this data

These instructions are defined in the pools runtime. Because data can look very different and every data stream has its unique features, other runtimes exist for different data streams. For example, to archive the Ethereum blockchain, the runtime will be @kyve/evm. Besides Ethereum, this runtime can also archive other EVM chains like Moonbeam or Aurora. For example, suppose you want to archive Solana. In that case, you need to run a different runtime specially designed for Solana data, @kyve/solana.

#### Funding

A storage pool requires funding in $KYVE and can be provided by anyone. The funding gets paid out to the protocol nodes active in the pool. If a pool runs out of funds, it stops. This is a crucial part of KYVEs token economics. The goal at KYVE is to build a decentralized data lake that gets utilized by as many users/projects as possible. When users create a business case on top of KYVE data, they are highly incentivized to ensure that the pool keeps producing the data. Whenever a pool is close to running out of tokens, it will purchase some more tokens and top up the pool's funding. The more users/projects do this, the more they share the costs, making it easier and reducing the risk of a pool running out of funding.

#### Staking

A storage pool requires protocol nodes that upload and validate data. To ensure that nodes upload correct data and validate honestly, the protocol nodes have to stake $KYVE. When protocol nodes stake $KYVE in a pool, they are allowed to operate in that specific pool. In case of nodes misbehaving (e.g., uploading and submitting invalid data or validating incorrectly), the node would get slashed. In return for the risk of being slashed and the work of uploading and validating data, nodes are rewarded with $KYVE based on their staking amount.

#### Delegating

By delegating to a node, you help to secure the network. Delegation is a form of staking which does not require you to run your node. You provide stake as network security to a node and generate rewards. In an ideal world, everyone would be able to run their node, leading to a very secure network with millions of nodes. But on the tech, this leads to many problems because those nodes generate a lot of traffic, leading the chain to slow down and eventually halt.

At KYVE, you can delegate to both protocol and chain nodes, allowing you to have multiple ways of earning rewards for your tokens.

### Subquery

### Summary

## Proposed design
