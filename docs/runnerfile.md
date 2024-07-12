# ctx-runner Module File

> Note: `Modulefile` syntax is in development
  default module `default.module` needed for entry point

A module file is the collection of multiple modules or single module defined in TOML to create runners/tasks for ctx-runners.

## Examples

### Basic `Modulefile`

An example of a `Modulefile` creating a simple get list of directories:

```modulefile
Name = 'cat'
Description = 'Gives contents of text file i.e, txt, html, json'

Exec = 'cat {{file}}'

[parameters]
Required = [ 'file' ]
Type = 'object'

[parameters.properties.file]
Description = 'absolute path of file'
Type = 'string'

+++

Name = 'ls'
Description = 'list of files in one or more directories'

Exec = '''
ls --format=single-column -A{{#each directories}} '{{this}}'{{/each}}
'''

[parameters]
Required = [ 'directories' ]
Type = 'object'

[parameters.properties.directories]
Description = 'an array of absolute paths of directories; i.e of path, /usr/home'
Items.Type = 'string'
Type = 'array'

+++

Name = 'view_image'
Description = 'view/show image file on current device'

Exec = '''
#!fork
swayimg --size=image '{{image}}'
'''

[parameters]
Required = [ 'image' ]
Type = 'object'

[parameters.properties.image]
Description = 'an absolute path of image; i.e of path, /parent/sub_parent/image.extension'
Type = 'string'
```

To use this:

- Save it as a file (e.g. `default.module`) in `$HOME/.config/ctxrn` dir.
- Start using the model!
