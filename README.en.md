### Introduction

2019 is approaching. The rust team keeps their promise about asynchronous IO: `async` is introduced as keywords, `Pin, Future, Poll` and `await!` is introduced into standard library. 

I have never used rust for asynchronous IO programming before so almost know nothing about it. However, I would use it for a project recently but couldn't find many documents that are remarkably helpful for newbie of rust asynchronous programming.

Eventually, I wrote several demo and implemented simple asynchronous IO based on `mio` and `coroutine` refered to this blog ([Tokio internals: Understanding Rust's asynchronous I/O framework from the bottom up](https://cafbit.com/post/tokio_internals/)) and "new tokio" [romio](https://github.com/withoutboats/romio)

