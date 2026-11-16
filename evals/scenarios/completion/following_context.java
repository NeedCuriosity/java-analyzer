class Main {
    private int count;
    private final String label = "x";

    int compute(int a) { return a; }

    void run(int limit, String text, int[] values) {
        int total = 0;
        /*@call*/(limit);
        this./*@field_assign*/ = 1;
        /*@local_assign*/ += limit;
        /*@deref*/.length();
        /*@index*/[0] = 1;
    }
}
//? complete call first compute
//? complete field_assign first count
//? complete local_assign order total compute
//? complete deref order text limit
//? complete index first values
