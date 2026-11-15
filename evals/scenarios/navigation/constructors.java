class Base {
    /*@base_default*/Base() {}
    /*@base_int*/Base(int v) {}
}

class Widget extends Base {
    final String name;

    /*@w_default*/Widget() {
        /*@this_call*/this("none");
    }

    /*@w_name*/Widget(String name) {
        /*@super_call*/super(1);
        this.name = name;
    }

    /*@w_two*/Widget(String name, int size) {
        this(name);
    }

    static Widget make() {
        Widget a = new /*@new_default*/Widget();
        Widget b = new /*@new_name*/Widget("x");
        return new /*@new_two*/Widget("y", 2);
    }
}
//? def this_call -> w_name
//? def super_call -> base_int
//? def new_default -> w_default
//? def new_name -> w_name
//? def new_two -> w_two
//? diag none
