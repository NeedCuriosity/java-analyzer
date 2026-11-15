class Main {
    int /*@field*/value = 1;

    int run(int /*@param*/value) {
        int a = /*@use_param*/value + this./*@use_field*/value;
        {
            int /*@block_local*/inner = a;
            a += /*@use_block*/inner;
        }
        for (int /*@for_var*/i = 0; /*@use_for*/i < 3; i++) {
            a += i;
        }
        Runnable r = new Runnable() {
            int /*@anon_field*/value = 5;
            public void run() { System.out.println(/*@use_anon*/value); }
        };
        r.run();
        return a;
    }

    int other() { return /*@use_field2*/value; }
}
//? def use_param -> param
//? def use_field -> field
//? def use_block -> block_local
//? def use_for -> for_var
//? def use_anon -> anon_field
//? def use_field2 -> field
//? diag none
