import java.io.IOException;
import java.util.concurrent.Callable;

class Main {
    void thrower() throws IOException {}

    interface Task {
        void run() throws IOException;
    }

    void anonymous() {
        Runnable r = new Runnable() {
            @Override
            public void run() {
                /*@anon*/thrower();
            }
        };
        r.run();
    }

    void local() {
        class Worker {
            Worker() {
                throw new /*@local_ctor*/IOException("x");
            }

            void work() {
                try {
                    thrower();
                } catch (IOException e) {
                    throw new IllegalStateException(e);
                }
            }
        }
        new Worker().work();
    }

    final Runnable field = new Runnable() {
        @Override
        public void run() {
            /*@in_field*/thrower();
        }
    };

    void ok1() {
        Task t = new Task() {
            @Override
            public void run() throws IOException {
                thrower();
            }
        };
        System.out.println(t);
    }

    void ok2() {
        Callable<Integer> c = new Callable<Integer>() {
            @Override
            public Integer call() throws Exception {
                thrower();
                return 1;
            }
        };
        System.out.println(c);
    }

    void ok3() {
        Runnable r = new Runnable() {
            @Override
            public void run() {
                try {
                    thrower();
                } catch (IOException e) {
                    throw new RuntimeException(e);
                }
            }
        };
        r.run();
    }

    void ok4() throws IOException {
        class Local {
            void work() throws IOException {
                thrower();
            }
        }
        new Local().work();
    }

    void ok5() {
        // 匿名类自己的 thrower() 不抛异常
        Runnable r = new Runnable() {
            void thrower() {}

            @Override
            public void run() {
                thrower();
            }
        };
        r.run();
    }
}
//? diag unhandled-exception anon
//? diag unhandled-exception local_ctor
//? diag unhandled-exception in_field
//? diag exact
