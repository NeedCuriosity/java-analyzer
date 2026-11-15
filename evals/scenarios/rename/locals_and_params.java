class Main {
    int run(int /*@param*/amount) {
        int /*@local*/total = amount * 2;
        Runnable r = () -> System.out.println(total + amount);
        r.run();
        int other = 1;
        return total + other;
    }
}
//? rename local sum ; has "int sum = amount * 2;" ; has "println(sum + amount)" ; has "return sum + other;" ; lacks "total"
//? rename param qty ; has "int run(int qty)" ; has "qty * 2" ; has "total + qty" ; lacks "amount"
//? rename local other ; fails
//? rename local amount ; fails
//? rename local 1bad ; fails
//? rename local class ; fails
