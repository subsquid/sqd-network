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

#### Data storage and validation

KYVE is an initiative to store any data stream with built-in validation. By leveraging the Arweave blockchain, we can permanently and immutably store this data.

**Data in rounds**

Saving many data items or even a data stream is tricky. Thats why we aggregate data into bundles to store them more efficiently. It enables KYVE to validate multiple data items which are bundled up in a single validation round. A selected validator will collect data in each round, create a bundle, and submit it to the network. This marks the beginning of such a proposal round.

**Lifecycle**

1. Selecting an uploader for the data bundle.
For a bundled proposal round to start, the next_uploader has to create a bundle, upload it to Arweave and then submit it to the network. But first, the next uploader has to be selected. For that, we use a weighted random selection with the following two factors:
    - The personal stake (linear)
    - The total delegation into the validator (sqrt)

2. Creating a bundle proposal. Once a node is selected as the next_uploader, it will create a bundle proposal that other validators can validate.

3. Uploading the bundle. Now the node can finally upload the bundle to Arweave. In return, it receives the transaction id of the Arweave transaction. The node can submit it to the network with this bundle id and other information like the `bundle_size` and the `byte_size`.

4. Validating the bundle. When the new bundle proposal is submitted, the latest round officially starts. The chain will emit an event that all nodes can listen to. When they see the uploader submitted a new bundle proposal, they will get the bundle id with the other data and start validating it. They take the bundle_id(the Arweave transaction id), download the raw bundle, unzip it, and parse the bundle to its original JSON format. After that the nodes will perform a simple hash compare. If the submitted byte_size is matching to the node will vote either valid or invalid.

5. Finalizing the bundle. After at least > 50% of all nodes (excluding the uploader) have voted quorum has been reached. If that is the case the uploader of the next round submits his bundle he prepared in the meantime and finalizes the round.

The bundle proposal was accepted and finalized if more than 50% of the validators voted with valid. The chain will save the bundle id and make it available for the users. As a reward, the uploader of this bundle will receive a bundle reward.

### Subquery

_The SubQuery Network indexes and services data to the global community in an incentivised and verifiable way_.

#### Consumers

A Consumer is a participant in the SubQuery network and is either an individual or organisation that pays for processed and organised blockchain data from the SubQuery Network. Consumers effectively make requests to the SubQuery Network for specific data and pay an agreed amount of SQT in return.

Consumers are typically dApp (decentralised application) developers, data analytic companies, blockchain networks, middleware developers, or even web aggregating companies that need access to blockchain data to provide services to their end-users.

The cost of querying data on the blockchain will be based on supply and demand and will be comparable to other similar services currently available. The advantage of an open and transparent network and ecosystem is that competition is encouraged to provide the best service to the Consumer.

#### Indexers

An Indexer is a SubQuery network participant who is responsible for indexing blockchain data and providing this data to their customers.

In order to earn rewards from query revenue as an Indexer it is proposed that Indexers must stake SQT against a particular SubQuery Project that they are providing the service to. The Cobb-Douglas production function will be used to determine the rewards distributed to each Indexer.

If an Indexer is caught misbehaving (such as by providing invalid, incomplete, or incorrect data), they are liable to have a portion of their staked SQT (on the particular reward pool ip) reallocated to the SubQuery Foundation Treasury, diminishing their holdings of staked SQT in the network and therefore their potential reward. Since the indexer’s allocated stake is determined by a percentage of their total SQT, this will have a flow on effect to all other reward pools that the indexer is party to.

Indexers are rewarded in SQT in two ways:
- Rewards from SQT reward pools based on distribution defined by the Cobb-Douglas Production Function.
- Direct SQT query fee rewards from Closed Agreements that an indexer is party to.

Indexers can increase their earning potential by attracting Delegators. Delegators are SQT token holders who can delegate their tokens to Indexers for additional rewards. Indexers use these additional tokens to increase the amount they allocate to projects of their choice. This allows Indexers to increase their earnings.

#### Delegators

A Delegator is a non-technical network role in the SubQuery Network and is a great way to start participating in the SubQuery Network. This role enables Delegators to “delegate” their SQT to one or more Indexers and earn rewards (similar to staking).

Without Delegators, Indexers will likely earn fewer rewards because they will have less SQT to allocate. Therefore, Indexers compete to attract Delegators by offering a competitive share of an Indexer’s rewards.

### Summary

## Proposed design
