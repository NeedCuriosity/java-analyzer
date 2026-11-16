class Main {
    int local() {
        class Counter {
            int /*@field*/hits;
            int /*@method*/bump() { return ++hits; }
            int twice() { return bump() + this.hits; }
        }
        Counter c = new Counter();
        c.hits = 1;
        Runnable r = () -> c.bump();
        return c.twice() + new Counter().hits;
    }

    int anonymous() {
        var acc = new Object() {
            int /*@total*/total;
            void /*@add*/add(int v) { total += v; }
        };
        acc.add(2);
        return acc.total;
    }
}
//? rename field count ; has "int count;" ; has "return ++count;" ; has "this.count" ; has "c.count = 1;" ; has "new Counter().count" ; lacks "hits"
//? rename method inc ; has "int inc() {" ; has "return inc() + this" ; has "() -> c.inc()" ; lacks "bump"
//? rename total sum ; has "int sum;" ; has "sum += v;" ; has "return acc.sum;" ; lacks "total"
//? rename add put ; has "void put(int v)" ; has "acc.put(2);" ; lacks "add"
