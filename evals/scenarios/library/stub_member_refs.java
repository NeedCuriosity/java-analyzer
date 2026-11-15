//- /org/gen/Generator.java lib:gen
package org.gen;

import java.util.List;

public class Generator {
    public Generator(Config config, List<String> warnings) {}
    public void generate(Callback callback) {}
    public void generate(Callback callback, boolean overwrite) {}
    public static Generator create() { return null; }
}
//- /org/gen/Config.java lib:gen
package org.gen;

public class Config {}
//- /org/gen/Callback.java lib:gen
package org.gen;

public interface Callback {}
//- /app/Main.java
package app;

import java.util.ArrayList;
import org.gen.Callback;
import org.gen.Config;
import org.gen./*@imp*/Generator;

class Main {
    void run(Callback callback) {
        /*@type_use*/Generator g = new /*@new_gen*/Generator(new Config(), new ArrayList<>());
        g./*@gen_call*/generate(callback);
        g./*@gen_call2*/generate(null, true);
        /*@type_use2*/Generator./*@create_call*/create();
    }
}
//? stub org.gen.Generator ; refs "Generator(Config" = new_gen ; refs "generate(Callback arg0)" = gen_call ; refs "generate(Callback arg0, boolean" = gen_call2 ; refs "create()" = create_call ; refs "Generator {" = imp type_use type_use2 new_gen
