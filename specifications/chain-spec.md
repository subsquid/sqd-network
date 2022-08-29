# Archive decentralization

## Abstract

## Subsquide team goals

## Similar approaches

### POKT Network

The [Pocket Network](pokt.md) is comprised of 3 components: Applications, Nodes and the Network Layer.

- An Application submits Relays, or API requests meant to be routed to any public database node.
- Nodes service these Relays, by submitting them to the public databases they are meant for, and sending the response (if any) back to the Application.
- The Network Layer is comprised of all the rules, protocols and finality storage that serve as the backbone of the interactions between Applications and Nodes, including (but not limited to), configuration, record tracking, governance and economic policy.
- The mechanism the Network uses to regulate the interactions between Applications and Nodes are Sessions.
- The only way to obtain sanctioned throughput from Nodes within the Pocket Network is by staking the native cryptocurrency within the network.
- Due to the architecture of the Session mechanism, Applications are provided five (5) individual Service Nodes per requested chain in one session.

### The Graph

[Graph Protocol](graph.md) falls into a category as a layer 2 read-scalability solution.

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

- the Query Node is responsible for discovering Indexing Nodes in the network that are indexing a specific dataset, and selecting an Indexing Node to read from based on factors such as price and performance (see Query Processing).
- Indexing Nodes index one or more user-defined datasets, called subgraphs. These nodes perform a deterministic streaming extract, transform and load (ETL) of events emitted by the Ethereum blockchain.
- Fisherman Services is an economic agent who verifies read responses in exchange for a reward in cases where they detect that an Indexing Node has attested to an incorrect response, and the Fisherman successfully disputes the response on-chain.
- Indexing Node operators stake deposits of Graph Tokens for particular datasets, called subgraphs, to gain the right to participate in the data retrieval marketplaces for that dataset-indexing data and responding to read requests in exchange for micropayments.
- Indexing Nodes which have staked to index a particular dataset, will be discoverable in the data retrieval market for that dataset.
- Curators are economic agents who earn rewards by betting on the future economic value of datasets, perhaps with the benefit of private information.
- Token holders who do not feel equipped to perform one of these functions may delegate their tokens to an Indexing Node that is staked for a particular dataset.
- In the Service Discovery step, the Query Node locates Indexing Nodes for a specific dataset as well as important metadata that is useful in deciding which Indexing Node to issue read operations to, such as price, performance, and economic security margin. Fetching price and latency for a node is done via a single call to the Indexing Node RPC API and returns the following data: the latency required to fulfill the request; a `bandwidthPrice` measured in price per byte transmitted over the network; and a `gasPrice`, which captures the cost of compute and IO for a given read operation.
- In the Service Selection stage, Query Nodes choose which Indexing Nodes to transact with for each read operation. An algorithm for this stage could incorporate latency (measured in ms), economicSecurityMargin (measured in Graph Tokens), gasPrice, and bytesPrice (the cost of sending a byte over the network).

### Kyve

[KYVE](kyve.md) is a network of storage pools built to store data streams or create snapshots of already existing data. It is secured by its blockchain built on cosmos.

![image](https://docs.kyve.network/architecture.png)

- The chain layer is an entirely sovereign Proof of Stake. This blockchain is run by independent nodes we call chain nodes since they're running on the chain level. The native currency of the KYVE chain is $KYVE. It secures the chain and allows chain nodes to stake and other users to delegate to them.
- The protocol layer sits on top of the chain layer and enables the actual use case of KYVE. Every feature and unit of logic which makes KYVE unique is implemented directly into the chain nodes. This includes pools, funding, staking and delegating.
- Storage pools (or just pools) can be described as discrete entities arranged around specific data sources. Protocol nodes have to run with the specified pool runtime for a pool to function. If specific criteria are met, pools distribute $KYVE to designated node runners.For example, to archive the Ethereum blockchain, the runtime will be @kyve/evm.
- A storage pool requires funding in $KYVE and can be provided by anyone. The funding gets paid out to the protocol nodes active in the pool. If a pool runs out of funds, it stops.
- A storage pool requires protocol nodes that upload and validate data. To ensure that nodes upload correct data and validate honestly, the protocol nodes have to stake $KYVE.
- Delegation is a form of staking which does not require you to run your node. You provide stake as network security to a node and generate rewards.

### Subquery

The [SubQuery Network](subquery.md) indexes and services data to the global community in an incentivised and verifiable way.

- A Consumer is a participant in the SubQuery network and is either an individual or organisation that pays for processed and organised blockchain data from the SubQuery Network.
- The cost of querying data on the blockchain will be based on supply and demand and will be comparable to other similar services currently available.
- An Indexer is a SubQuery network participant who is responsible for indexing blockchain data and providing this data to their customers.
- In order to earn rewards from query revenue as an Indexer it is proposed that Indexers must stake SQT against a particular SubQuery Project that they are providing the service to.
- A Delegator is a non-technical network role in the SubQuery Network and is a great way to start participating in the SubQuery Network. This role enables Delegators to “delegate” their SQT to one or more Indexers and earn rewards (similar to staking).

### Summary

## Proposed design

In assumption, that archives business logic is going to manage different stuff like some rules being a part of archives set, regulating and validatation network members activities, economic logic enabling, etc. The general subsquid-network should should consists of 2 key layers, similar to Kyve:
- The chain layer.
- The protocol layer.

### The chain layer

The chain layer is responsible mainly for maintaining decentralized peer-to-peer network that operates the same data storage for the state changes recorded as a result of transactions, consensus methodology for blocks production and finalization to protect against malicious activity and ensure the ongoing progress of the chain, cryptography logic to enable blockchain works in the way we need.

Additionally, as we know, running and maintaining such logic requires resources - processors, memory, storage, and network bandwidth—to perform operations. To support a community and make the network sustainable, users should pay for the network resources they use in different forms that will be defined.

#### Core components

- _Blockchain core state_ that defines transaction execution and application rules.
- _Validators_ are responsible for block production and finalization to maintain state changes properly.
- _Squid token_ to manage economic in terms of token transfer, fees, staking, delegation, etc.

### The protocol layer

The protocol layer is responsible for defining and managing subsquid specific related activities. Logically we should define what is an Archive as a network member, it's goals and rules; who is able to be a part of Archive set; different techniques and mechanisms to process and validate client requests to archives, rewards and punishment logic to stimulate protocol members do their job properly without behaving badly.

### Archive

Archive is a service that ingests raw on-chain data, stores it into persistent storage in a normalized way and exposes it via API for downstream data pipelines (such as Squid Processor) and ad-hoc exploration. Compared to data access using a conventional chain node RPC, an archive allows one to access data in a more granular fashion and from multiple blocks at once, thanks to its rich batching and filtering capabilities.

### Archive logical requirements

- Archive is run for particular blockchain network.
- Archive should be able to process data extraction requests and return a proper response that will be validated. Requests and responses should meet a predefined format.
- Archive owner should stake a minimum required subsquid tokens to join the Archive set.
- Archive owner should pass a registration process to join the Archive set.
- Archive should get reward for valid work.
- Archive misbehavior should be punished.

### Requests types

We should define a light request in some way. It's like a request that can be processed by one archive using predefined resources and times. The requests that require more resources or time are Heavy requests.

#### Request processing

Requests are submitted to the network on-chain (via transaction). The network devides the request into chunks of light requests if the request is heavy. Then the network should choose a set of archive that should process corresponding light request based on some metrics and prices. The request has a special status until it has been processing.

When the archive is ready to provide response to submmitted light request then it submits the response via a transaction as well. The response can contain some proof of processed data that link to any distributed storage like IPFS.

The network validate responses and does punishment logic if it's required.

### Pools

Archive with the same target blockchain can be united into Pools. The Pool is able to process more than 1 light requests as it consists of some number of archives. The Pool should define their resources, times, prices, number of light requests that can be proccesed in 1 unit request time. It's required to properly choose a set of archives and pool that will process incoming request by the network.

The pool owner defines the pool rewards, the canonical image for the archive.

### Aka Fisherman service

It's a network member that run a service that meets requirements to validate archive and pool responses and in the event of an invalid response, may run a protocol-level dispute.

### Staking

To ensure that archives process requests honestly, the archives have to stake subsquid tokens. In case of misbehaving (e.g., uploading and submitting invalid data), the archive or pool would get slashed.

### Delegating

Delegation is a form of staking which does not require you to run your node or archive. Uou can delegate to both validators and archives, allowing you to have multiple ways of earning rewards for your tokens.

## Implementation key thoughts

Before going in depth about the archive decentralized implementation itself we would like to consider basic approaches of decentralization logic implementation. There are the following keys options to enable it:

- _Develop on top of existing blockchain_. Building on top of an existing blockchain comes with many advantages. As an extant blockchain would already be fully functional, the only bits to worry about would be implementing our custom logic. However, the most meaningful limit for us is scaling. In this case, the chain layer should operate considered blockchain with it's rules. It doesn't allow to customize token economics as it always should rely on existing native economic as well.
- _Write own blockchain from scratch_. This approach allows maximum flexibility in how to use blockchain. But it's extremely expensive that requires to spend a lot of time to security related tasks that are not a part of our main focus. Additionally, there are hard problems around scale, governance, interoperability, and upgradeability to address.
- _Use a blockchain framework_. A blockchain framework is a kind blockchain that already provides key blockchains componenents that easly can be customized based on you needs. These offerings with peering, consensus, blockchain storage, and other important systems built–in, while allowing you to plug in your own custom transactions, verification rules, and state. There are some of them like Hyperledfer family frameworks, R3 Corda, Substrate, etc.

Based on it we decided that using a blockchain framework suits us the best based on our subsquid goals as we don't aim to overcame obstacles in terms of general blockchain tasks.

After speding some time on blockchain framework research, we decided to use a Substrate as it provides the following great features: [flexible](https://docs.substrate.io/fundamentals/why-substrate/#flexible), [open](https://docs.substrate.io/fundamentals/why-substrate/#open), [interoperable](https://docs.substrate.io/fundamentals/why-substrate/#interoperable) and [future-proof](https://docs.substrate.io/fundamentals/why-substrate/#future-proof).
